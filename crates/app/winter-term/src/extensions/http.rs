//! Extensions' web requests ([`Host::fetch`](winter_ext_api::Host::fetch)),
//! made through `curl`: it ships with Linux, macOS, and Windows 10 on, and
//! carries its own TLS.
//!
//! The headers, which carry the tokens, go to `curl` on its standard input
//! (`-H @-`) rather than on its command line, where any other process on the
//! machine could read them. A header naming one of the extension's secrets
//! has it filled in here, so the secret never reaches the extension.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use winter_base::process;
use winter_ext_api::{FetchError, Request, Response};

use super::secret;

// ========================================================================
// Constants
// ========================================================================

/// The program the requests are made through.
const CURL: &str = "curl";

/// The only scheme a request goes out on.
const HTTPS: &str = "https://";

/// How long a whole request may take before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Printed after the body, on a line of its own, so the status can be told
/// from the body without a second request.
const STATUS_TRAILER: &str = "\n%{http_code}";

// ========================================================================
// Functions
// ========================================================================

/// Make `request` for the extension `extension`, giving back the answer
/// whatever its status. Run off the window's thread: it waits on the
/// network.
pub(crate) fn send(extension: &str, request: &Request) -> Result<Response, FetchError> {
    if !request.url.starts_with(HTTPS) {
        return Err(FetchError::NotAllowed);
    }
    let headers = header_lines(extension, request)?;
    let mut command = process::command(CURL);
    command
        .args(["--silent", "--max-time"])
        .arg(TIMEOUT.as_secs().to_string())
        .args(["--proto", "=https", "--header", "@-"])
        .args(["--write-out", STATUS_TRAILER]);
    if let Some(body) = &request.body {
        command.args(["--request", "POST", "--data-binary", body]);
    }
    let mut child = command
        .arg(&request.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| FetchError::Failed)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A few short lines: they fit the pipe whole, so writing them before
        // reading what comes back cannot deadlock.
        let _ = stdin.write_all(headers.join("\n").as_bytes());
    }
    let output = child.wait_with_output().map_err(|_| FetchError::Failed)?;
    split_status(&String::from_utf8_lossy(&output.stdout))
}

/// `request`'s headers as whole lines, `Name: value`, each secret they name
/// filled in from `extension`'s.
fn header_lines(extension: &str, request: &Request) -> Result<Vec<String>, FetchError> {
    request
        .headers
        .iter()
        .map(|header| {
            let secret = match &header.secret {
                Some(key) => secret::get(extension, key)
                    .ok_or_else(|| FetchError::MissingSecret(key.clone()))?,
                None => String::new(),
            };
            Ok(format!("{}: {}{secret}", header.name, header.value))
        })
        .collect()
}

/// The answer in what `curl` printed: its body, then its status.
fn split_status(printed: &str) -> Result<Response, FetchError> {
    let (body, code) = printed.rsplit_once('\n').ok_or(FetchError::Failed)?;
    // `curl` reports 000 when nothing answered.
    match code.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err(FetchError::Failed),
        Ok(status) => Ok(Response::new(status, body)),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_status_after_the_body_is_the_answer_s() {
        assert_eq!(
            split_status("{\"a\":1}\n200"),
            Ok(Response::new(200, "{\"a\":1}"))
        );
        assert_eq!(
            split_status("denied\n401"),
            Ok(Response::new(401, "denied"))
        );
        assert_eq!(split_status("\n000"), Err(FetchError::Failed));
        assert_eq!(split_status(""), Err(FetchError::Failed));
    }

    #[test]
    fn test_a_request_off_https_never_goes_out() {
        let plain = Request::new("http://example.com/");
        assert_eq!(send("probe", &plain), Err(FetchError::NotAllowed));
    }
}
