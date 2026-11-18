//! What a command reads when it runs inside the `mcp` server.
//!
//! A command run from a shell owns the process: it writes to stdout, reads
//! stdin when asked to, and builds an HTTP client it will use once. Inside the
//! server none of that holds. Stdout and stdin carry the protocol, several
//! tool calls can be open at once, and the point of a long-lived process is
//! that the second call reuses the connection the first one opened.
//!
//! So each tool call runs with a [`Call`] set for the length of its task. The
//! three places a command touches the process, the output stream, the `-`
//! input and the HTTP client, ask here first. Outside the server nothing is
//! set and every one of them behaves as it always has.
//!
//! The state is task local and the runtime is single threaded, so nothing here
//! locks.

use std::cell::RefCell;
use std::future::Future;
use std::io::Write;
use std::rc::Rc;

tokio::task_local! {
    static CALL: Call;
}

/// What one tool call carries.
struct Call {
    http: reqwest::Client,
    input: Option<String>,
    output: Rc<RefCell<Vec<u8>>>,
}

/// Where a command's results go while a tool call is open.
pub struct Sink(Rc<RefCell<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Whether this command is running as a tool call.
pub fn active() -> bool {
    CALL.try_with(|_| ()).is_ok()
}

/// The server's shared HTTP client, when a tool call is open.
pub fn http_client() -> Option<reqwest::Client> {
    CALL.try_with(|call| call.http.clone()).ok()
}

/// The stream that stands in for stdout, when a tool call is open.
pub fn sink() -> Option<Sink> {
    CALL.try_with(|call| Sink(Rc::clone(&call.output))).ok()
}

/// The text that stands in for stdin, when a tool call is open and has any.
pub fn input() -> Option<String> {
    CALL.try_with(|call| call.input.clone()).ok().flatten()
}

/// Run one command as a tool call and hand back what it wrote.
pub async fn scope<F: Future>(
    http: reqwest::Client,
    input: Option<String>,
    command: F,
) -> (F::Output, Vec<u8>) {
    let output = Rc::new(RefCell::new(Vec::new()));
    let call = Call {
        http,
        input,
        output: Rc::clone(&output),
    };
    let result = CALL.scope(call, command).await;
    let written = std::mem::take(&mut *output.borrow_mut());
    (result, written)
}
