//! The wire a language server speaks: JSON-RPC messages, each a JSON body
//! after a `Content-Length` header, over the server's standard input and
//! output.

use std::io::{self, BufRead, Write};

use serde_json::Value;

// ========================================================================
// Constants
// ========================================================================

/// The header that says how long the body after it is.
const LENGTH_HEADER: &str = "content-length";

/// The longest body read, past which the stream is taken as broken rather
/// than a message: a server that wrote this much in one message is not
/// talking LSP.
const MAX_BODY: usize = 256 * 1024 * 1024;

// ========================================================================
// Functions
// ========================================================================

/// Write `message` to `out` as one framed message.
pub(super) fn write_message(out: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body.as_bytes())?;
    out.flush()
}

/// Read the next framed message from `input`, or `None` once the stream has
/// ended between messages. A header without a length, or a body that is
/// not JSON, is an error: the stream cannot be resynchronized past it.
pub(super) fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return match length {
                None => Ok(None),
                Some(_) => Err(broken("the stream ended inside a header")),
            };
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case(LENGTH_HEADER) {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length.ok_or_else(|| broken("a header without a length"))?;
    if length > MAX_BODY {
        return Err(broken("a body too long to be a message"));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| broken(&format!("a body that is not JSON: {e}")))
}

/// An error for a stream that no longer reads as messages.
fn broken(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("language server sent {what}"))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_messages_written_read_back_in_order_until_the_stream_ends() {
        let mut wire = Vec::new();
        write_message(&mut wire, &json!({"id": 1, "result": "é"})).expect("write");
        write_message(&mut wire, &json!({"method": "exit"})).expect("write");
        let mut input = io::Cursor::new(wire);
        assert_eq!(read_message(&mut input).expect("read"), Some(json!({"id": 1, "result": "é"})));
        assert_eq!(read_message(&mut input).expect("read"), Some(json!({"method": "exit"})));
        assert_eq!(read_message(&mut input).expect("end"), None);
    }

    #[test]
    fn test_other_headers_are_skipped_and_a_missing_length_is_an_error() {
        let wire = b"Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\n{}";
        let mut input = io::Cursor::new(wire.to_vec());
        assert_eq!(read_message(&mut input).expect("read"), Some(json!({})));
        let mut input = io::Cursor::new(b"Content-Type: x\r\n\r\n{}".to_vec());
        assert!(read_message(&mut input).is_err());
    }
}
