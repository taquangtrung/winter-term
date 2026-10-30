//! HTTP requests, made through `curl`: it ships with Linux, macOS, and
//! Windows 10 on, carries its own TLS, and takes a self-signed certificate
//! when told to, which Antigravity's local server has.
//!
//! The headers, which carry the tokens, go to `curl` on its standard input
//! (`-H @-`) rather than on its command line, where any other process on the
//! machine could read them.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use crate::process;

// ========================================================================
// Constants
// ========================================================================

/// The program the requests are made through.
const CURL: &str = "curl";

/// Printed after the body, on a line of its own, so the status can be told
/// from the body without a second request.
const STATUS_TRAILER: &str = "\n%{http_code}";

// ========================================================================
// Data Structures
// ========================================================================

/// One request to make.
pub(super) struct Request<'a> {
    /// Sent as a POST when given; a GET is made otherwise.
    pub(super) body: Option<&'a str>,
    /// Whole header lines, `Name: value`.
    pub(super) headers: &'a [String],
    /// Whether a certificate that does not check out is taken anyway: only
    /// for a server on this machine.
    pub(super) insecure: bool,
    /// How long the whole request may take before it is given up on.
    pub(super) timeout: Duration,
    pub(super) url: &'a str,
}

/// Why a request came to nothing.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum HttpError {
    /// The server answered, with this status.
    Status(u16),
    /// No answer at all: `curl` missing, no connection, or out of time.
    Unanswered,
}

// ========================================================================
// Functions
// ========================================================================

/// Make `request`, giving back the body of a 2xx answer.
pub(super) fn send(request: &Request) -> Result<String, HttpError> {
    let mut command = process::command(CURL);
    command
        .args(["--silent", "--max-time"])
        .arg(request.timeout.as_secs().to_string())
        .args(["--header", "@-", "--write-out", STATUS_TRAILER]);
    if request.insecure {
        command.arg("--insecure");
    }
    if let Some(body) = request.body {
        command.args(["--request", "POST", "--data-binary", body]);
    }
    let mut child = command
        .arg(request.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| HttpError::Unanswered)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A few short lines: they fit the pipe whole, so writing them before
        // reading what comes back cannot deadlock.
        let _ = stdin.write_all(request.headers.join("\n").as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|_| HttpError::Unanswered)?;
    split_status(&String::from_utf8_lossy(&output.stdout))
}

/// The body of what `curl` printed, when the status after it is a 2xx.
fn split_status(printed: &str) -> Result<String, HttpError> {
    let (body, code) = printed.rsplit_once('\n').ok_or(HttpError::Unanswered)?;
    // `curl` reports 000 when nothing answered.
    match code.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err(HttpError::Unanswered),
        Ok(status) if (200..300).contains(&status) => Ok(body.to_string()),
        Ok(status) => Err(HttpError::Status(status)),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_status_after_the_body_decides_the_outcome() {
        assert_eq!(split_status("{\"a\":1}\n200"), Ok("{\"a\":1}".to_string()));
        assert_eq!(split_status("denied\n401"), Err(HttpError::Status(401)));
        assert_eq!(split_status("\n000"), Err(HttpError::Unanswered));
        assert_eq!(split_status(""), Err(HttpError::Unanswered));
    }
}
