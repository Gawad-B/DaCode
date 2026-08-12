//! JSON-RPC framing over a pipe.
//!
//! The wire format is `Content-Length: N\r\n\r\n<N bytes of JSON>`. Small, but unforgiving: the
//! length is in **bytes**, not characters, and a server that sends a header vex mishandles will
//! desynchronize the stream permanently rather than fail visibly.
//!
//! Generic over `Read`/`Write` so the tests can drive it through in-memory buffers instead of a
//! child process.

use anyhow::{Context, Result, bail};
use std::io::{BufRead, Write};

/// Frames a JSON message for transmission.
pub fn encode(message: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("a JSON value always serializes");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

pub fn write_message(writer: &mut impl Write, message: &serde_json::Value) -> Result<()> {
    writer
        .write_all(&encode(message))
        .context("writing to the language server")?;
    // Without the flush the server waits for data that is sitting in our buffer, and the session
    // deadlocks with both sides idle.
    writer.flush().context("flushing to the language server")
}

/// Reads one framed message. `Ok(None)` means the stream ended cleanly.
pub fn read_message(reader: &mut impl BufRead) -> Result<Option<serde_json::Value>> {
    let mut length: Option<usize> = None;

    // Headers, terminated by a blank line.
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).context("reading a header")?;
        if read == 0 {
            return Ok(None); // the server closed its output
        }

        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        let Some((name, value)) = line.split_once(':') else {
            bail!("malformed header line: {line:?}");
        };
        // Header names are case-insensitive per the spec, and servers differ.
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .trim()
                    .parse()
                    .with_context(|| format!("bad Content-Length: {value:?}"))?,
            );
        }
    }

    let length = length.context("a message arrived with no Content-Length")?;
    let mut body = vec![0u8; length];
    std::io::Read::read_exact(reader, &mut body).context("reading a message body")?;

    let value = serde_json::from_slice(&body).with_context(|| {
        format!(
            "body was not valid JSON: {:?}",
            String::from_utf8_lossy(&body)
        )
    })?;
    Ok(Some(value))
}
