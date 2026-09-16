//! Asking where the newest release is, and fetching its files.
//!
//! This is its own HTTP client and not the Spider API client. A release check
//! carries no key, spends no credits, and does not care whether the API is up.
//!
//! "Latest" is read from the redirect `github.com/<repo>/releases/latest`
//! answers with, rather than from `api.github.com`. The API allows sixty
//! unauthenticated calls an hour per address, and a machine behind a shared
//! address can spend those without ever running this tool. The redirect is
//! the web page, answers with the tag in its `Location`, and the asset URLs
//! follow from the naming contract, so no JSON is needed either. A 403 or 429
//! from either is still read as a rate limit and turns into a quiet skip.

use std::time::Duration;

use url::Url;

use minisign_verify::PublicKey;

use super::{archive, signature};
use super::{Problem, Settings, Version};

/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one request may take, body included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How many redirects a download may follow. GitHub uses one, to its asset
/// storage host.
const MAX_REDIRECTS: usize = 5;

/// A release: its tag as written, and the version the tag names.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: Version,
}

/// Whether a URL may be fetched at all.
///
/// Only https, with one exception that exists for tests: plain http to a
/// loopback IP address when the settings allow it, which only a debug build's
/// settings can (see `Settings`).
pub fn allowed(url: &Url, allow_loopback_http: bool) -> bool {
    match url.scheme() {
        "https" => true,
        "http" if allow_loopback_http => match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(_)) | None => false,
        },
        _ => false,
    }
}

/// The two clients: one that follows no redirect, for reading where latest
/// points, and one that follows redirects only while each hop is allowed.
pub struct Releases {
    base: Url,
    allow_loopback_http: bool,
    fetcher: reqwest::Client,
    pointer: reqwest::Client,
}

impl Releases {
    pub fn new(settings: &Settings) -> Result<Releases, Problem> {
        let allow = settings.allow_loopback_http;
        if !allowed(&settings.base, allow) {
            return Err(Problem::Unreachable(format!(
                "refusing the release address {}, which is not https",
                settings.base
            )));
        }
        let user_agent = concat!("spider-agent/", env!("CARGO_PKG_VERSION"));
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if allowed(attempt.url(), allow) {
                attempt.follow()
            } else {
                attempt.error("a redirect left https")
            }
        });
        let build = |policy| {
            reqwest::Client::builder()
                .user_agent(user_agent)
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .redirect(policy)
                .build()
                .map_err(|error| Problem::Local(format!("could not build an HTTP client: {error}")))
        };
        Ok(Releases {
            base: settings.base.clone(),
            allow_loopback_http: allow,
            fetcher: build(policy)?,
            pointer: build(reqwest::redirect::Policy::none())?,
        })
    }

    /// The page for a release, for a person to open.
    pub fn page(&self, tag: &str) -> String {
        format!("{}/tag/{tag}", self.base.as_str().trim_end_matches('/'))
    }

    fn under_base(&self, path: &str) -> Result<Url, Problem> {
        let text = format!("{}/{path}", self.base.as_str().trim_end_matches('/'));
        let url = Url::parse(&text)
            .map_err(|error| Problem::Local(format!("could not build {text}: {error}")))?;
        self.check(url)
    }

    fn check(&self, url: Url) -> Result<Url, Problem> {
        if allowed(&url, self.allow_loopback_http) {
            Ok(url)
        } else {
            Err(Problem::Unreachable(format!(
                "refusing {url}, which is not https"
            )))
        }
    }

    /// The newest published release.
    pub async fn latest(&self) -> Result<Release, Problem> {
        let url = self.under_base("latest")?;
        let response = self
            .pointer
            .get(url.clone())
            .send()
            .await
            .map_err(|error| Problem::Unreachable(format!("could not reach {url}: {error}")))?;
        let status = response.status();
        if is_rate_limit(status) {
            return Err(Problem::RateLimited);
        }
        if !status.is_redirection() {
            return Err(Problem::Unreachable(format!(
                "{url} answered {status} where a redirect to the newest release was expected"
            )));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| Problem::Unreachable(format!("{url} redirected nowhere")))?;
        let target = url.join(location).map_err(|error| {
            Problem::Unreachable(format!(
                "{url} redirected to an unreadable address: {error}"
            ))
        })?;
        let target = self.check(target)?;
        let tag = tag_from(&target)
            .ok_or_else(|| Problem::Unreachable(format!("{target} does not name a release tag")))?;
        let version = Version::parse(tag.strip_prefix('v').unwrap_or(&tag)).ok_or_else(|| {
            Problem::Broken(format!("the newest release tag {tag} is not a version"))
        })?;
        Ok(Release { tag, version })
    }

    /// One file attached to a release, up to `cap` bytes.
    pub async fn download(&self, tag: &str, file: &str, cap: usize) -> Result<Vec<u8>, Problem> {
        self.download_if_present(tag, file, cap)
            .await?
            .ok_or_else(|| Problem::Unreachable(format!("{file} is not attached to release {tag}")))
    }

    /// The same, with `None` for a file the release does not have.
    pub async fn download_if_present(
        &self,
        tag: &str,
        file: &str,
        cap: usize,
    ) -> Result<Option<Vec<u8>>, Problem> {
        let url = self.under_base(&format!("download/{tag}/{file}"))?;
        let mut response = self
            .fetcher
            .get(url.clone())
            .send()
            .await
            .map_err(|error| Problem::Unreachable(format!("could not download {file}: {error}")))?;
        let status = response.status();
        if is_rate_limit(status) {
            return Err(Problem::RateLimited);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(Problem::Unreachable(format!(
                "{file} could not be downloaded: {url} answered {status}"
            )));
        }
        let too_big = || Problem::Broken(format!("{file} is larger than this tool will download"));
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(cap).unwrap_or(u64::MAX))
        {
            return Err(too_big());
        }
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len().saturating_add(chunk.len()) > cap {
                        return Err(too_big());
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => {
                    return Err(Problem::Unreachable(format!(
                        "the download of {file} broke off: {error}"
                    )))
                }
            }
        }
        Ok(Some(body))
    }
}

fn is_rate_limit(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// The segment after `tag` in `.../releases/tag/<tag>`.
fn tag_from(url: &Url) -> Option<String> {
    let segments: Vec<&str> = url.path_segments()?.collect();
    let position = segments.iter().rposition(|segment| *segment == "tag")?;
    let tag = segments.get(position + 1)?;
    (!tag.is_empty()).then(|| (*tag).to_string())
}

/// The asset for this platform, by the naming contract.
pub fn asset_name(version: &Version, triple: &str) -> String {
    format!("spider-agent-{version}-{triple}.tar.gz")
}

/// Download the checksum file, its signature and the asset, and hand back the
/// binary inside only if a release key signed the checksum file for this
/// version and the archive matched it.
pub async fn fetch_verified(
    releases: &Releases,
    keys: &[PublicKey],
    release: &Release,
    triple: &str,
) -> Result<Vec<u8>, Problem> {
    let asset = asset_name(&release.version, triple);
    let sums = releases
        .download(&release.tag, "SHA256SUMS.txt", archive::MAX_SUMS_BYTES)
        .await?;
    // A release with no signature is refused, not skipped: an attacker who
    // can publish unsigned files must not get a quiet retry every day.
    let minisig = releases
        .download_if_present(
            &release.tag,
            signature::SIGNATURE_FILE,
            signature::MAX_SIGNATURE_BYTES,
        )
        .await?
        .ok_or_else(|| {
            Problem::Mismatch(format!(
                "release {} has no {}",
                release.tag,
                signature::SIGNATURE_FILE
            ))
        })?;
    // Nothing in the sums file is read until the signature over it holds.
    signature::verify_sums(&sums, &minisig, keys, &release.version).map_err(Problem::Mismatch)?;
    let expected = archive::expected_digest(&sums, &asset).map_err(Problem::Broken)?;
    let bytes = releases
        .download(&release.tag, &asset, archive::MAX_ARCHIVE_BYTES)
        .await?;
    let actual = archive::sha256(&bytes);
    if actual != expected {
        return Err(Problem::Mismatch(format!(
            "{asset} did not match SHA256SUMS.txt (expected {}, got {})",
            archive::hex(&expected),
            archive::hex(&actual)
        )));
    }
    // Only now, with the bytes known to be the ones the release lists, is the
    // archive opened.
    archive::extract_binary(&bytes).map_err(|reason| {
        Problem::Broken(format!("{asset} from release {}: {reason}", release.tag))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn only_https_is_fetched_unless_loopback_http_is_allowed() {
        let https = Url::parse("https://github.com/x").unwrap();
        let loopback = Url::parse("http://[::1]:8080/x").unwrap();
        let named = Url::parse("http://example.com:8080/x").unwrap();
        let remote_http = Url::parse("http://example.com/x").unwrap();
        let file = Url::parse("file:///etc/passwd").unwrap();
        assert!(allowed(&https, false));
        assert!(!allowed(&loopback, false));
        assert!(allowed(&loopback, true));
        assert!(!allowed(&named, true));
        assert!(!allowed(&remote_http, true));
        assert!(!allowed(&file, true));
    }

    #[test]
    fn the_tag_is_read_from_the_redirect() {
        let url = Url::parse("https://github.com/spider-rs/spider-cloud-agent/releases/tag/v0.4.1")
            .unwrap();
        assert_eq!(tag_from(&url).as_deref(), Some("v0.4.1"));
        let bare = Url::parse("https://github.com/spider-rs/spider-cloud-agent/releases").unwrap();
        assert_eq!(tag_from(&bare), None);
    }

    #[test]
    fn the_asset_name_follows_the_contract() {
        let version = Version::parse("0.4.0").unwrap();
        assert_eq!(
            asset_name(&version, "aarch64-apple-darwin"),
            "spider-agent-0.4.0-aarch64-apple-darwin.tar.gz"
        );
    }
}
