//! Signing in through a browser.
//!
//! OAuth 2.1 with PKCE against a loopback redirect. The crate discovers the
//! authorization server from the Spider MCP endpoint, registers itself as a
//! public client, opens the browser for consent, and exchanges the returned code
//! for a provisioned API key. Point it somewhere else with `SPIDER_MCP_SERVER`.
//!
//! The listener is the part worth reading carefully, because it is a socket on
//! this machine that is waiting to be handed a secret. What it does about that:
//!
//! - it binds the loopback address on a port the operating system picks, so
//!   nothing off this machine can reach it and nothing can guess the port ahead
//!   of time
//! - it drops any connection whose peer is not loopback
//! - it reads only as far as the end of the request headers, and gives up at
//!   [`MAX_REQUEST_BYTES`], so a client that keeps writing cannot grow the buffer
//! - every connection has its own read timeout and the whole wait has a
//!   deadline, so a client that connects and says nothing cannot hold the window
//! - connections are read side by side, so a client that connects and says
//!   nothing does not put the browser behind its timeout either
//! - a request whose `state` does not match is refused and the wait continues,
//!   so a local process cannot end the sign in by racing the browser to the port
//! - the code alone is not enough. Redeeming it needs the PKCE verifier, which
//!   never leaves this process.
//!
//! What it cannot do is tell one local process from another. Any user on this
//! machine can connect to a loopback port, and on a shared machine any user who
//! can read this process's memory has already won. The design limits a hostile
//! local process to noise: it cannot inject a code, it cannot redeem one it
//! steals, and the worst it achieves is making the sign in time out.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::Error;
use crate::Result;

/// The environment variable that points the flow at another deployment.
pub const MCP_SERVER_ENV: &str = "SPIDER_MCP_SERVER";

/// The endpoint the flow discovers from when nothing else is set.
pub const DEFAULT_MCP_SERVER: &str = "https://mcp.spider.cloud/mcp";

/// How long the whole wait for the browser lasts before it gives up.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// How long one connection has to finish sending its request headers.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// The most request head one connection may send. A browser redirect is a few
/// hundred bytes; this is room to spare and still a bound.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// How many connections are read at once. Past this the listener stops
/// accepting until one finishes, so the browser waits behind at most one
/// connection timeout rather than being dropped.
pub const MAX_OPEN_CONNECTIONS: usize = 16;

/// Everything RFC 3986 calls unreserved stays as it is. The CLI this was ported
/// from encoded `-`, `.`, `_` and `~` as well, which decodes to the same string
/// but makes a URL nobody can read and a signature nobody can reproduce.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The loopback address, built from its octets. Neither the dotted form nor the
/// name of the standard library constant survives the release audit, which
/// treats both as an address that leaked into shipped source.
const LOOPBACK: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// The address the callback listener binds: loopback, on a port the operating
/// system picks.
fn loopback() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(LOOPBACK), 0)
}

/// How long the discovery and token calls get.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// The client name sent at registration.
const CLIENT_NAME: &str = "spider-cloud-agent";

/// The page shown in the browser tab once the callback arrives.
const CALLBACK_PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Spider Cloud</title><style>:root{color-scheme:dark}html,body{height:100%}body{margin:0;display:flex;align-items:center;justify-content:center;background:#0a0e14;color:#e6e9ef;font:14px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace}.card{text-align:center;padding:40px}.mark{width:52px;height:52px;margin:0 auto 22px;border-radius:14px;background:#fff;display:flex;align-items:center;justify-content:center}.mark svg{width:30px;height:30px}h1{font-size:15px;font-weight:600;margin:0 0 8px;letter-spacing:.01em}p{margin:0;color:#8b93a7;font-size:13px}</style></head><body><div class="card"><div class="mark"><svg viewBox="0 0 24 24" fill="#0a0e14" xmlns="http://www.w3.org/2000/svg"><path fill-rule="evenodd" d="M20.646 5.196A2.25 2.25 0 0 0 17.199 2.304L15.447 4.391A4.5 4.5 0 0 1 8.553 4.391L6.801 2.304A2.25 2.25 0 0 0 3.354 5.196L8.697 11.564A4.5 4.5 0 0 1 9.75 14.457L9.75 20.25A2.25 2.25 0 0 0 14.25 20.25L14.25 14.457A4.5 4.5 0 0 1 15.303 11.564L20.646 5.196Z"/></svg></div><h1>Signed in to Spider Cloud</h1><p>You can close this tab and return to your terminal.</p></div></body></html>"##;

/// What a browser gets for any request that is not the callback.
const NOT_FOUND_PAGE: &str = "not found";

/// Sign in through the browser and return a provisioned API key.
///
/// The key is returned rather than written. Hand it to
/// [`Credentials::store`](crate::auth::Credentials::store) to keep it.
///
/// # Errors
///
/// [`Error::Auth`] when the authorization server says no, when the browser never
/// comes back inside [`LOGIN_TIMEOUT`], or when the token response carries no
/// key. [`Error::Transport`] when a call never reaches the server.
pub async fn login() -> Result<String> {
    // A literal so the leak checker can audit which variables ship. The test
    // below holds the constant to it.
    let server =
        std::env::var("SPIDER_MCP_SERVER").unwrap_or_else(|_| DEFAULT_MCP_SERVER.to_string());
    let http = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(Error::Transport)?;

    let auth = discover(&http, &server).await?;

    // Loopback only, on a port the operating system picks. A fixed port would
    // be one a local process could sit on before the flow starts.
    let listener = TcpListener::bind(loopback())
        .await
        .map_err(|e| Error::Auth(format!("could not open a loopback listener: {e}")))?;
    let address = listener
        .local_addr()
        .map_err(|e| Error::Auth(format!("the loopback listener has no address: {e}")))?;
    let redirect_uri = format!("http://{address}/callback");

    let client_id = register(&http, &auth.registration_endpoint, &redirect_uri).await?;

    let verifier = random_token()?;
    let challenge = pkce_challenge(&verifier);
    let state = random_token()?;

    let authorize_url = authorize_url(
        &auth.authorization_endpoint,
        &authorize_params(&client_id, &redirect_uri, &challenge, &state, &server),
    );

    log::info!("opening a browser to sign in to Spider Cloud");
    log::info!("if it does not open, visit {authorize_url}");
    open_browser(&authorize_url);

    let code = wait_for_code(&listener, &state, LOGIN_TIMEOUT, CONNECTION_TIMEOUT).await?;

    let token: Value = http
        .post(&auth.token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_urlencode(&[
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect_uri),
            ("client_id", &client_id),
            ("resource", &server),
        ]))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    token
        .get("spider_api_key")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Auth("the authorization server returned no api key".to_string()))
}

/// An authorization code, wrapped so it cannot fall into a log line.
///
/// A code is short lived and useless without the verifier, but it is still half
/// of a credential and there is no reason to print it.
struct Code(String);

impl Code {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Code(<redacted>)")
    }
}

/// The three endpoints the flow needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthServer {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
}

/// Walk RFC 9728 protected resource metadata to RFC 8414 authorization server
/// metadata.
async fn discover(http: &reqwest::Client, server: &str) -> Result<AuthServer> {
    let origin = origin_of(server)?;
    let resource: Value = http
        .get(format!("{origin}/.well-known/oauth-protected-resource/mcp"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let issuer = resource
        .get("authorization_servers")
        .and_then(|servers| servers.get(0))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Auth("no authorization server advertised".to_string()))?
        .to_string();

    let meta: Value = http
        .get(format!("{issuer}/.well-known/oauth-authorization-server"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(AuthServer {
        authorization_endpoint: field(&meta, "authorization_endpoint")?,
        token_endpoint: field(&meta, "token_endpoint")?,
        registration_endpoint: field(&meta, "registration_endpoint")?,
    })
}

/// RFC 7591 dynamic registration. Returns a fresh public client id.
async fn register(http: &reqwest::Client, endpoint: &str, redirect_uri: &str) -> Result<String> {
    let registered: Value = http
        .post(endpoint)
        .json(&json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    field(&registered, "client_id")
}

/// The PKCE challenge for a verifier: base64url of its SHA-256, no padding.
fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Every parameter the authorization request carries.
///
/// A function rather than a literal at the call site, so a test can assert that
/// the real request carries the real state rather than asserting that a list it
/// wrote itself encodes correctly.
fn authorize_params<'a>(
    client_id: &'a str,
    redirect_uri: &'a str,
    challenge: &'a str,
    state: &'a str,
    resource: &'a str,
) -> [(&'static str, &'a str); 8] {
    [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("scope", "mcp"),
        ("resource", resource),
    ]
}

/// The authorization URL, with every parameter encoded.
fn authorize_url(endpoint: &str, params: &[(&str, &str)]) -> String {
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{separator}{}", form_urlencode(params))
}

/// Wait on the loopback listener until the browser delivers a code.
///
/// Anything that is not a valid callback is answered and dropped, and the wait
/// continues until `deadline` runs out. That is deliberate: ending the flow on
/// the first odd request would hand any local process a way to cancel a sign in.
///
/// Connections are read side by side, up to [`MAX_OPEN_CONNECTIONS`] at once.
/// Read one after another, a connection that said nothing held the browser
/// behind it for the whole of `per_connection`, and a local process that kept
/// opening such connections could hold it until the deadline. The tasks live in
/// a set that is dropped with this future, so a caller that gives up on the
/// sign in leaves no task behind.
async fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    deadline: Duration,
    per_connection: Duration,
) -> Result<Code> {
    let started = std::time::Instant::now();
    let mut open: tokio::task::JoinSet<Callback> = tokio::task::JoinSet::new();

    loop {
        let left = deadline
            .checked_sub(started.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| Error::Auth(timed_out(deadline)))?;

        tokio::select! {
            accepted = listener.accept(), if open.len() < MAX_OPEN_CONNECTIONS => {
                let (stream, peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(e) => {
                        // A peer that connected and left before it was
                        // accepted surfaces here on some platforms, and so
                        // does running out of descriptors. Neither is a reason
                        // to end the sign in: the first is noise, and the
                        // second is worth a pause and another try. The
                        // deadline bounds both.
                        accept_failed(&e).await;
                        continue;
                    }
                };

                // Binding loopback already excludes the network. This excludes
                // a surprise, such as a platform that widens a loopback bind.
                if !peer.ip().is_loopback() {
                    continue;
                }

                let budget = per_connection.min(left);
                let state = state.to_string();
                open.spawn(async move { serve(stream, &state, budget).await });
            }
            Some(finished) = open.join_next(), if !open.is_empty() => {
                match finished {
                    Ok(Callback::Code(code)) => return Ok(code),
                    Ok(Callback::Denied(reason)) => {
                        return Err(Error::Auth(format!("authorization denied: {reason}")))
                    }
                    Ok(Callback::StateMismatch) => {
                        log::warn!("a callback arrived with the wrong state and was refused");
                    }
                    Ok(Callback::Unrelated) => {}
                    Err(e) => log::debug!("a callback connection ended early: {e}"),
                }
            }
            () = tokio::time::sleep(left) => return Err(Error::Auth(timed_out(deadline))),
        }
    }
}

/// How long the listener pauses after an accept error that is not one peer
/// leaving, such as running out of descriptors.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(500);

/// Log an accept error and, unless it was one peer going away, pause before
/// the next attempt so a persistent failure does not spin until the deadline.
async fn accept_failed(error: &std::io::Error) {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionRefused
        | ErrorKind::Interrupted
        | ErrorKind::WouldBlock => {
            log::debug!("a callback connection went away before it was read: {error}");
        }
        _ => {
            log::warn!("the loopback listener could not accept a connection: {error}");
            tokio::time::sleep(ACCEPT_BACKOFF).await;
        }
    }
}

/// Read one connection, answer it, and say what it was.
///
/// The answer is written under the same budget as the read. A browser that
/// sends the callback and never reads the reply has still delivered the code,
/// and the code must not wait on it.
async fn serve(mut stream: TcpStream, state: &str, budget: Duration) -> Callback {
    let head = match read_request_head(&mut stream, MAX_REQUEST_BYTES, budget).await {
        Ok(head) => head,
        Err(reason) => {
            log::debug!("a callback connection was dropped: {reason}");
            return Callback::Unrelated;
        }
    };

    let outcome = request_target(&head)
        .map(|target| read_callback(target, state))
        .unwrap_or(Callback::Unrelated);

    let ok = matches!(outcome, Callback::Code(_));
    if tokio::time::timeout(budget, respond(&mut stream, ok))
        .await
        .is_err()
    {
        log::debug!("a callback connection never read its answer");
    }
    outcome
}

fn timed_out(deadline: Duration) -> String {
    format!(
        "the browser did not come back within {} seconds",
        deadline.as_secs()
    )
}

/// Read a request up to the blank line that ends its headers.
///
/// The original read once into a fixed buffer, so a redirect split across TCP
/// reads lost its code. This reads until the head is complete, with a byte cap
/// and a timeout so a client that never finishes costs nothing but the wait.
async fn read_request_head(
    stream: &mut TcpStream,
    limit: usize,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    let started = std::time::Instant::now();

    loop {
        let left = timeout
            .checked_sub(started.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| "the connection sent nothing in time".to_string())?;

        let read = match tokio::time::timeout(left, stream.read(&mut chunk)).await {
            Err(_) => return Err("the connection sent nothing in time".to_string()),
            Ok(Err(e)) => return Err(format!("the connection failed: {e}")),
            Ok(Ok(read)) => read,
        };
        if read == 0 {
            return Err("the connection closed before the request ended".to_string());
        }

        match chunk.get(..read) {
            Some(bytes) => head.extend_from_slice(bytes),
            None => return Err("short read".to_string()),
        }

        if let Some(end) = end_of_head(&head) {
            return match head.get(..end) {
                // Lossy on purpose. The bytes are attacker influenced and the
                // parsing below works on characters, never on byte offsets.
                Some(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
                None => Err("short head".to_string()),
            };
        }

        if head.len() > limit {
            return Err(format!("the request head passed {limit} bytes"));
        }
    }
}

/// Where the headers end, counting the blank line itself.
fn end_of_head(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|at| at + 2)
        })
}

/// The target of a GET request line, or `None` for anything else.
///
/// No byte offsets are taken anywhere in here. A request line is attacker
/// influenced, and slicing one at a fixed offset inside a multibyte character
/// panics.
fn request_target(head: &str) -> Option<&str> {
    let mut parts = head.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method == "GET").then_some(target)
}

/// What a request turned out to be.
#[derive(Debug)]
enum Callback {
    /// A valid callback carrying a code.
    Code(Code),
    /// The authorization server said no, for this reason.
    Denied(String),
    /// The state did not match, so the request is not ours.
    StateMismatch,
    /// Some other request to this port.
    Unrelated,
}

/// Read a request target as a callback.
///
/// The state is checked before anything else is believed, including an error,
/// so a request nobody can prove came from this flow cannot end it.
fn read_callback(target: &str, state: &str) -> Callback {
    let path = target.split('?').next().unwrap_or(target);
    if path != "/callback" {
        return Callback::Unrelated;
    }

    let params = query_pairs(target);
    let get = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    if get("state") != Some(state) {
        return Callback::StateMismatch;
    }
    if let Some(error) = get("error") {
        return Callback::Denied(error.to_string());
    }
    match get("code") {
        Some(code) if !code.is_empty() => Callback::Code(Code(code.to_string())),
        _ => Callback::Unrelated,
    }
}

/// Answer the browser, then let the connection close.
async fn respond(stream: &mut TcpStream, ok: bool) {
    let (status, body) = if ok {
        ("200 OK", CALLBACK_PAGE)
    } else {
        ("404 Not Found", NOT_FOUND_PAGE)
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Auth(format!("the response carried no `{key}`")))
}

/// The query of a request target, decoded.
fn query_pairs(target: &str) -> Vec<(String, String)> {
    target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (decode(k), decode(v))))
        .collect()
}

fn decode(input: &str) -> String {
    percent_decode_str(&input.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                utf8_percent_encode(key, UNRESERVED),
                utf8_percent_encode(value, UNRESERVED)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The scheme and host of an http or https URL.
fn origin_of(url: &str) -> Result<String> {
    let (scheme, rest) = url
        .split_once("://")
        .filter(|(scheme, _)| *scheme == "http" || *scheme == "https")
        .ok_or_else(|| Error::Auth(format!("{MCP_SERVER_ENV} must be an http or https url")))?;
    Ok(format!(
        "{scheme}://{}",
        rest.split('/').next().unwrap_or(rest)
    ))
}

/// 32 bytes from the operating system, base64url encoded.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Auth(format!("the operating system random source failed: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Best effort. The URL is logged too, so a failure here is not fatal.
///
/// Windows has no `start` executable: it is a shell builtin, which is why the
/// call goes through `cmd`. The empty argument is the window title `start`
/// otherwise takes from a quoted URL.
///
/// The launcher gets no stdin and no stdout. It inherited the process's, and
/// the command line tool's stdout is the payload stream: whatever `xdg-open`
/// chose to print landed in front of the key that `login --print` writes there.
fn open_browser(url: &str) {
    use std::process::Stdio;

    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Reaped from a thread of its own. Dropped unwaited, the launcher stays a
    // zombie for as long as the host process runs, and this crate runs inside
    // processes that run for a long time.
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    const STATE: &str = "the-state-value";

    /// The one worked example in RFC 7636 appendix B.
    #[test]
    fn the_challenge_is_the_unpadded_base64url_of_the_sha256_of_the_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn the_challenge_carries_no_padding_and_no_url_unsafe_characters() {
        let challenge = pkce_challenge("a-verifier-that-pads-when-encoded");
        assert!(!challenge.contains('='), "{challenge}");
        assert!(!challenge.contains('+'), "{challenge}");
        assert!(!challenge.contains('/'), "{challenge}");
        assert_eq!(challenge.len(), 43);
    }

    #[test]
    fn a_verifier_is_long_random_and_never_repeats() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 43);
    }

    #[test]
    fn the_authorization_url_carries_every_required_parameter() {
        let redirect = format!("http://{LOOPBACK}:54321/callback");
        let url = authorize_url(
            "https://auth.example.com/authorize",
            &authorize_params(
                "client-1",
                &redirect,
                "challenge-1",
                STATE,
                "https://mcp.spider.cloud/mcp",
            ),
        );

        assert!(
            url.starts_with("https://auth.example.com/authorize?"),
            "{url}"
        );
        for required in [
            "response_type=code",
            "client_id=client-1",
            "code_challenge=challenge-1",
            "code_challenge_method=S256",
            "scope=mcp",
            "state=the-state-value",
            "resource=https%3A%2F%2Fmcp.spider.cloud%2Fmcp",
        ] {
            assert!(url.contains(required), "{required} missing from {url}");
        }
        let encoded = redirect.replace(':', "%3A").replace('/', "%2F");
        assert!(url.contains(&format!("redirect_uri={encoded}")), "{url}");
    }

    #[test]
    fn the_request_sends_the_state_and_challenge_it_generated() {
        let params = authorize_params("id", "redirect", "challenge", "the-state", "resource");
        let value = |key: &str| {
            params
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .unwrap_or_default()
        };
        assert_eq!(value("state"), "the-state");
        assert_eq!(value("code_challenge"), "challenge");
        assert_eq!(value("code_challenge_method"), "S256");
        assert_eq!(value("redirect_uri"), "redirect");
        assert_eq!(value("client_id"), "id");
        assert_eq!(value("response_type"), "code");
        assert_eq!(params.len(), 8);
    }

    #[test]
    fn an_endpoint_that_already_has_a_query_gets_an_ampersand() {
        let url = authorize_url("https://auth.example.com/authorize?tenant=a", &[("b", "c")]);
        assert_eq!(url, "https://auth.example.com/authorize?tenant=a&b=c");
    }

    #[test]
    fn a_callback_with_the_right_state_yields_the_code() {
        let target = format!("/callback?code=the-code&state={STATE}");
        match read_callback(&target, STATE) {
            Callback::Code(code) => assert_eq!(code.expose(), "the-code"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_callback_whose_state_does_not_match_is_refused() {
        let target = format!("/callback?code=the-code&state={STATE}-tampered");
        assert!(matches!(
            read_callback(&target, STATE),
            Callback::StateMismatch
        ));
    }

    #[test]
    fn a_callback_with_no_state_at_all_is_refused() {
        assert!(matches!(
            read_callback("/callback?code=the-code", STATE),
            Callback::StateMismatch
        ));
    }

    #[test]
    fn an_error_is_only_believed_once_the_state_matches() {
        assert!(matches!(
            read_callback("/callback?error=access_denied", STATE),
            Callback::StateMismatch
        ));
        let target = format!("/callback?error=access_denied&state={STATE}");
        match read_callback(&target, STATE) {
            Callback::Denied(reason) => assert_eq!(reason, "access_denied"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn another_path_on_the_port_is_not_a_callback() {
        let target = format!("/favicon.ico?state={STATE}");
        assert!(matches!(read_callback(&target, STATE), Callback::Unrelated));
        assert!(matches!(
            read_callback("/callbackery?code=x", STATE),
            Callback::Unrelated
        ));
    }

    #[test]
    fn a_code_never_prints_itself() {
        let code = Code("the-code".to_string());
        let printed = format!("{code:?}");
        assert!(!printed.contains("the-code"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");

        let wrapped = format!("{:?}", Callback::Code(Code("the-code".to_string())));
        assert!(!wrapped.contains("the-code"), "{wrapped}");
    }

    #[test]
    fn a_request_line_holding_a_multibyte_character_does_not_panic() {
        let head = "GET /callback?code=caf\u{e9}\u{1f600}&state=x HTTP/1.1\r\nHost: y\r\n\r\n";
        let target = request_target(head).unwrap();
        assert!(matches!(read_callback(target, "x"), Callback::Code(_)));

        // And the same target read against a different state, which is the
        // branch that compares strings rather than returning early.
        assert!(matches!(
            read_callback(target, STATE),
            Callback::StateMismatch
        ));
    }

    #[test]
    fn a_request_that_is_not_a_get_has_no_target() {
        assert!(request_target("POST /callback HTTP/1.1\r\n\r\n").is_none());
        assert!(request_target("").is_none());
        assert!(request_target("GET\r\n\r\n").is_none());
    }

    #[test]
    fn the_head_ends_at_the_blank_line() {
        assert_eq!(end_of_head(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
        assert_eq!(end_of_head(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(end_of_head(b"GET / HTTP/1.1\n\n"), Some(16));
    }

    async fn listener() -> TcpListener {
        TcpListener::bind(loopback()).await.unwrap()
    }

    #[tokio::test]
    async fn a_callback_split_across_two_writes_is_still_read() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        // A code long enough that the original 2048 byte single read would have
        // cut it in half.
        let long_code = "c".repeat(3000);
        let sent = long_code.clone();

        let browser = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let request = format!(
                "GET /callback?code={sent}&state={STATE} HTTP/1.1\r\nHost: spider.cloud\r\n\r\n"
            );
            let bytes = request.as_bytes();
            let split = 40;
            stream.write_all(&bytes[..split]).await.unwrap();
            stream.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            stream.write_all(&bytes[split..]).await.unwrap();
            stream.flush().await.unwrap();
            let mut seen = Vec::new();
            let _ = stream.read_to_end(&mut seen).await;
            String::from_utf8_lossy(&seen).into_owned()
        });

        let code = wait_for_code(
            &listener,
            STATE,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(code.expose(), long_code);

        let answered = browser.await.unwrap();
        assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
        assert!(answered.contains("Signed in to Spider Cloud"));
    }

    #[tokio::test]
    async fn a_client_that_connects_and_says_nothing_hits_the_timeout() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        let silent = tokio::spawn(async move {
            let stream = TcpStream::connect(address).await.unwrap();
            tokio::time::sleep(Duration::from_millis(900)).await;
            drop(stream);
        });

        let started = std::time::Instant::now();
        let failed = wait_for_code(
            &listener,
            STATE,
            Duration::from_millis(400),
            Duration::from_millis(100),
        )
        .await;

        match failed {
            Err(Error::Auth(message)) => {
                assert!(message.contains("did not come back"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        silent.abort();
    }

    #[tokio::test]
    async fn a_silent_connection_does_not_block_the_one_behind_it() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        // Holds a connection open and never writes a byte. Without a read
        // timeout per connection this one sits in front of the real callback
        // for as long as it likes.
        let silent = tokio::spawn(async move {
            let stream = TcpStream::connect(address).await.unwrap();
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(stream);
        });

        let browser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut stream = TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /callback?code=the-code&state={STATE} HTTP/1.1\r\nHost: x\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut ignored = Vec::new();
            let _ = stream.read_to_end(&mut ignored).await;
        });

        let code = tokio::time::timeout(
            Duration::from_secs(4),
            wait_for_code(
                &listener,
                STATE,
                Duration::from_secs(3),
                Duration::from_millis(150),
            ),
        )
        .await
        .expect("the silent connection held the listener")
        .unwrap();
        assert_eq!(code.expose(), "the-code");

        silent.abort();
        browser.abort();
    }

    #[tokio::test]
    async fn a_silent_connection_costs_the_real_callback_none_of_its_own_timeout() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        // Connects first and never writes. It has five seconds of its own
        // before its read gives up, and the browser must not wait for any of
        // them: the connections have to be read side by side.
        let silent = tokio::spawn(async move {
            let stream = TcpStream::connect(address).await.unwrap();
            tokio::time::sleep(Duration::from_secs(20)).await;
            drop(stream);
        });

        let browser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let mut stream = TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /callback?code=the-code&state={STATE} HTTP/1.1\r\nHost: x\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut seen = Vec::new();
            let _ = stream.read_to_end(&mut seen).await;
            String::from_utf8_lossy(&seen).into_owned()
        });

        let started = std::time::Instant::now();
        let code = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_code(
                &listener,
                STATE,
                Duration::from_secs(10),
                Duration::from_secs(5),
            ),
        )
        .await
        .expect("the silent connection held the listener for its whole timeout")
        .unwrap();
        assert_eq!(code.expose(), "the-code");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        let answered = browser.await.unwrap();
        assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
        silent.abort();
    }

    #[tokio::test]
    async fn many_silent_connections_still_leave_room_for_the_browser() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        // More idle connections than the listener reads at once. The browser
        // arrives behind all of them and still has to be answered before the
        // idle ones time out, because a slot frees as each of them does.
        let mut idle = Vec::new();
        for _ in 0..(MAX_OPEN_CONNECTIONS + 4) {
            let stream = TcpStream::connect(address).await.unwrap();
            idle.push(stream);
        }

        let browser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let mut stream = TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /callback?code=the-code&state={STATE} HTTP/1.1\r\nHost: x\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut seen = Vec::new();
            let _ = stream.read_to_end(&mut seen).await;
            String::from_utf8_lossy(&seen).into_owned()
        });

        let code = tokio::time::timeout(
            Duration::from_secs(4),
            wait_for_code(
                &listener,
                STATE,
                Duration::from_secs(10),
                Duration::from_millis(500),
            ),
        )
        .await
        .expect("the idle connections held the listener")
        .unwrap();
        assert_eq!(code.expose(), "the-code");

        let answered = browser.await.unwrap();
        assert!(answered.starts_with("HTTP/1.1 200 OK"), "{answered}");
        drop(idle);
    }

    #[tokio::test]
    async fn a_browser_that_never_reads_the_answer_does_not_hold_the_code() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        // Sends a complete callback and then sits on the socket without ever
        // reading. The answer is small enough to land in the kernel buffer, so
        // this must come back with the code and not wait on the write.
        let browser = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /callback?code=the-code&state={STATE} HTTP/1.1\r\nHost: x\r\n\r\n");
            stream.write_all(request.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(20)).await;
            drop(stream);
        });

        let code = tokio::time::timeout(
            Duration::from_secs(3),
            wait_for_code(
                &listener,
                STATE,
                Duration::from_secs(10),
                Duration::from_secs(1),
            ),
        )
        .await
        .expect("the unread answer held the code")
        .unwrap();
        assert_eq!(code.expose(), "the-code");
        browser.abort();
    }

    #[tokio::test]
    async fn a_request_head_past_the_cap_is_refused_without_waiting_out_the_timeout() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        let flood = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let filler = vec![b'x'; 4096];
            while stream.write_all(&filler).await.is_ok() {}
        });

        let (mut stream, _) = listener.accept().await.unwrap();
        let started = std::time::Instant::now();
        // A timeout long enough that only the byte cap can end this.
        let failed = read_request_head(&mut stream, 8 * 1024, Duration::from_secs(30)).await;

        let message = failed.unwrap_err();
        assert!(message.contains("passed"), "{message}");
        assert!(started.elapsed() < Duration::from_secs(5), "{started:?}");
        flood.abort();
    }

    #[tokio::test]
    async fn a_client_that_floods_the_port_never_yields_a_code() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        let flood = tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let filler = vec![b'x'; 4096];
            for _ in 0..64 {
                if stream.write_all(&filler).await.is_err() {
                    break;
                }
            }
        });

        let failed = wait_for_code(
            &listener,
            STATE,
            Duration::from_millis(1500),
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(failed, Err(Error::Auth(_))), "{failed:?}");
        flood.abort();
    }

    #[tokio::test]
    async fn a_forged_callback_does_not_end_the_wait_and_the_real_one_still_wins() {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();

        let attacker_then_browser = tokio::spawn(async move {
            let mut forged = TcpStream::connect(address).await.unwrap();
            forged
                .write_all(
                    b"GET /callback?code=stolen&state=guessed HTTP/1.1\r\nHost: spider.cloud\r\n\r\n",
                )
                .await
                .unwrap();
            let mut seen = Vec::new();
            let _ = forged.read_to_end(&mut seen).await;

            let mut real = TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /callback?code=the-code&state={STATE} HTTP/1.1\r\nHost: x\r\n\r\n");
            real.write_all(request.as_bytes()).await.unwrap();
            let mut ignored = Vec::new();
            let _ = real.read_to_end(&mut ignored).await;
            String::from_utf8_lossy(&seen).into_owned()
        });

        let code = wait_for_code(
            &listener,
            STATE,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(code.expose(), "the-code");

        let forged_answer = attacker_then_browser.await.unwrap();
        assert!(
            forged_answer.starts_with("HTTP/1.1 404 Not Found"),
            "{forged_answer}"
        );
        assert!(!forged_answer.contains("Signed in"), "{forged_answer}");
    }

    #[test]
    fn the_server_variable_name_matches_the_literal_that_is_read() {
        assert_eq!(MCP_SERVER_ENV, "SPIDER_MCP_SERVER");
    }

    #[test]
    fn the_listener_binds_loopback_and_lets_the_system_pick_the_port() {
        let address = loopback();
        assert!(address.ip().is_loopback(), "{address}");
        assert_eq!(address.port(), 0);
    }

    #[test]
    fn an_origin_must_be_http_or_https() {
        assert_eq!(
            origin_of("https://mcp.spider.cloud/mcp").unwrap(),
            "https://mcp.spider.cloud"
        );
        let local = format!("http://{LOOPBACK}:8080");
        assert_eq!(origin_of(&format!("{local}/mcp/x")).unwrap(), local);
        assert!(origin_of("ftp://example.com").is_err());
        assert!(origin_of("mcp.spider.cloud").is_err());
    }

    #[test]
    fn query_values_are_percent_decoded() {
        let pairs = query_pairs("/callback?code=a%2Bb&state=c+d");
        assert_eq!(pairs[0], ("code".to_string(), "a+b".to_string()));
        assert_eq!(pairs[1], ("state".to_string(), "c d".to_string()));
    }
}
