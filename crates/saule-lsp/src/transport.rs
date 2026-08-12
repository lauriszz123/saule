//! Hardening for the stdin side of the LSP connection.
//!
//! `tower-lsp` treats a malformed frame as fatal in two separate ways, and a
//! single bad byte from the editor triggers both:
//!
//! * It answers with a JSON-RPC error carrying `"id": null` and no `method`.
//!   That message is unclassifiable — it is neither a request, a response, nor
//!   a notification — so lsp4j's `MessageTypeAdapter` parses it to `null` and
//!   LSP4IJ's `handleLSPMessage(@NotNull message)` throws
//!   `IllegalArgumentException` before it ever reaches a handler. The user
//!   sees "Error while handling LSP message of the language server 'saule'".
//! * The `serve` loop then *ends*, so the process exits with status 0 and every
//!   feature dies until the editor restarts it.
//!
//! Neither is recoverable from inside a `LanguageServer` impl, because both
//! happen below it — the message never gets far enough to be dispatched. So the
//! bad frame has to be stopped before `tower-lsp` sees it. [`sanitize`] re-frames
//! the incoming stream and forwards only messages that are guaranteed to
//! dispatch: valid UTF-8, valid JSON, a JSON object, and carrying either a
//! `method` or a non-null `id`. Anything else is reported on stderr — where the
//! editor's LSP console shows it — and skipped, leaving the connection intact.
//!
//! Dropping a malformed message silently loses it, and a *request* lost this way
//! leaves the client waiting for a reply it will never get. That is deliberate:
//! the id cannot be recovered from a body that does not parse, so there is
//! nobody to answer, and clients already time out pending requests. Losing one
//! request beats losing the whole session.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// How much header text to tolerate before concluding the stream is
/// desynchronised. Real header blocks are well under 100 bytes.
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Ceiling on a single message. Generous — a `didOpen` of a large file is a
/// legitimate multi-megabyte frame — but bounded, so a corrupt length field
/// cannot make us reserve arbitrary memory.
const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;

/// Read framed LSP messages from `input` and write the well-formed ones to
/// `output`, discarding the rest.
///
/// Returns when `input` reaches EOF. Dropping `output` afterwards is what tells
/// the server to shut down, so the caller should let it fall out of scope.
pub async fn sanitize<R, W>(mut input: R, mut output: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut chunk = vec![0u8; 16 * 1024];

    loop {
        let n = input.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);

        // One read can complete several frames, or none.
        loop {
            match step(&mut buf) {
                Step::NeedMore => break,
                Step::Dropped => continue,
                Step::Frame(body) => {
                    output
                        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                        .await?;
                    output.write_all(&body).await?;
                    output.flush().await?;
                }
            }
        }
    }

    Ok(())
}

/// The outcome of trying to pull one frame off the front of `buf`.
enum Step {
    /// Not enough bytes yet; `buf` is untouched.
    NeedMore,
    /// Something unusable was consumed. Try again — there may be more behind it.
    Dropped,
    /// A message that will dispatch.
    Frame(Vec<u8>),
}

fn step(buf: &mut Vec<u8>) -> Step {
    let (headers_end, content_len) = match parse_headers(buf) {
        Headers::Incomplete => {
            // A header block this long means we are reading something that is
            // not a header block at all, and no amount of waiting will fix it.
            if buf.len() > MAX_HEADER_BYTES {
                warn("header block exceeded 64 KiB; discarding buffered input");
                buf.clear();
                return Step::Dropped;
            }
            return Step::NeedMore;
        }
        Headers::Complete { end, len } => (end, len),
    };

    let Some(content_len) = content_len else {
        warn("frame has no usable Content-Length header; skipping it");
        buf.drain(..headers_end);
        return Step::Dropped;
    };

    if content_len > MAX_BODY_BYTES {
        warn(&format!(
            "Content-Length {content_len} exceeds the {MAX_BODY_BYTES}-byte ceiling; skipping"
        ));
        buf.drain(..headers_end);
        return Step::Dropped;
    }

    if buf.len() < headers_end + content_len {
        return Step::NeedMore;
    }

    let body: Vec<u8> = buf[headers_end..headers_end + content_len].to_vec();
    buf.drain(..headers_end + content_len);

    match dispatchable(&body) {
        Ok(()) => Step::Frame(body),
        Err(why) => {
            warn(&format!("dropping an undispatchable message: {why}"));
            Step::Dropped
        }
    }
}

enum Headers {
    Incomplete,
    Complete {
        /// Offset of the first body byte.
        end: usize,
        /// `None` when the block carried no parsable `Content-Length`.
        len: Option<usize>,
    },
}

/// Parse the header block at the front of `buf`.
///
/// Header names are matched case-insensitively: HTTP's rules apply here, and a
/// client that sends `content-length` is within its rights even though every
/// common one sends `Content-Length`. Both CRLF and bare LF terminate a line,
/// because clients in the wild send both.
fn parse_headers(buf: &[u8]) -> Headers {
    let mut pos = 0;
    let mut len = None;

    loop {
        let Some(nl) = buf[pos..].iter().position(|&b| b == b'\n') else {
            return Headers::Incomplete;
        };
        let line_end = pos + nl;
        let mut line = &buf[pos..line_end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        pos = line_end + 1;

        // The blank line ends the block.
        if line.is_empty() {
            return Headers::Complete { end: pos, len };
        }

        let Some(colon) = line.iter().position(|&b| b == b':') else {
            // Not a header line at all. Keep scanning: the blank line is still
            // the only thing that ends the block, and giving up here would
            // leave the body to be reinterpreted as headers.
            continue;
        };
        let name = &line[..colon];
        if name.eq_ignore_ascii_case(b"content-length") {
            len = std::str::from_utf8(&line[colon + 1..])
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok());
        }
    }
}

/// Whether `body` is something `tower-lsp` can route without erroring out.
///
/// The `method`-or-non-null-`id` test is exactly the classification lsp4j and
/// `tower-lsp` both perform: a request has both, a response has an id, a
/// notification has a method. An object with neither is the unclassifiable
/// shape that starts this whole failure mode, so it is rejected here rather
/// than reflected back as an `"id": null` error.
fn dispatchable(body: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(body).map_err(|e| format!("not valid UTF-8 ({e})"))?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?;
    let serde_json::Value::Object(map) = value else {
        return Err("not a JSON object".into());
    };

    let has_method = map.get("method").is_some_and(serde_json::Value::is_string);
    let has_id = map.get("id").is_some_and(|id| !id.is_null());
    if has_method || has_id {
        Ok(())
    } else {
        Err("no `method` and no non-null `id`".into())
    }
}

/// Report on stderr. stdout is the protocol channel and must never carry
/// anything but frames — writing diagnostics there is the very corruption this
/// module exists to contain.
fn warn(message: &str) {
    eprintln!("saule-lsp: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> Vec<u8> {
        let mut v = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        v.extend_from_slice(body.as_bytes());
        v
    }

    async fn run(input: Vec<u8>) -> String {
        let mut out: Vec<u8> = Vec::new();
        sanitize(&input[..], &mut out).await.unwrap();
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn forwards_a_well_formed_message() {
        let out = run(frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)).await;
        assert_eq!(
            out,
            "Content-Length: 46\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}"
        );
    }

    #[tokio::test]
    async fn forwards_a_notification() {
        let out = run(frame(r#"{"jsonrpc":"2.0","method":"initialized"}"#)).await;
        assert!(out.contains(r#""method":"initialized""#));
    }

    /// The regression that matters: a bad frame must not take the good frame
    /// behind it down with it. Before this module, the first one ended the
    /// session.
    #[tokio::test]
    async fn a_bad_frame_does_not_swallow_the_next_one() {
        let mut input = frame("{not json at all");
        input.extend(frame(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#));
        let out = run(input).await;
        assert!(!out.contains("not json"), "malformed body was forwarded");
        assert!(out.contains(r#""method":"shutdown""#), "good frame was lost");
    }

    #[tokio::test]
    async fn drops_invalid_utf8() {
        let body = b"{\"jsonrpc\":\"2.0\",\"method\":\"x\",\"params\":\"\xff\xfe\"}";
        let mut input = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        input.extend_from_slice(body);
        input.extend(frame(r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#));
        let out = run(input).await;
        assert!(out.contains(r#""id":3"#));
        assert_eq!(out.matches("Content-Length").count(), 1);
    }

    /// The exact shape that crashes LSP4IJ, arriving from the other direction.
    #[tokio::test]
    async fn drops_an_object_with_neither_method_nor_id() {
        let out = run(frame(r#"{"jsonrpc":"2.0","error":{"code":-32700}}"#)).await;
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn drops_an_explicitly_null_id_with_no_method() {
        let out = run(frame(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-1}}"#)).await;
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn keeps_a_response_carrying_only_an_id() {
        let out = run(frame(r#"{"jsonrpc":"2.0","id":7,"result":null}"#)).await;
        assert!(out.contains(r#""id":7"#));
    }

    #[tokio::test]
    async fn drops_a_non_object_body() {
        let out = run(frame("[1,2,3]")).await;
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn accepts_lowercase_header_names() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let mut input = format!("content-length: {}\r\n\r\n", body.len()).into_bytes();
        input.extend_from_slice(body.as_bytes());
        let out = run(input).await;
        assert!(out.contains(r#""method":"initialize""#));
    }

    #[tokio::test]
    async fn accepts_a_content_type_header_and_lf_line_endings() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let mut input = format!(
            "Content-Length: {}\nContent-Type: application/vscode-jsonrpc; charset=utf-8\n\n",
            body.len()
        )
        .into_bytes();
        input.extend_from_slice(body.as_bytes());
        let out = run(input).await;
        assert!(out.contains(r#""method":"initialize""#));
    }

    #[tokio::test]
    async fn skips_a_frame_with_no_content_length() {
        let mut input = b"X-Nonsense: 1\r\n\r\n".to_vec();
        input.extend(frame(r#"{"jsonrpc":"2.0","id":4,"method":"shutdown"}"#));
        let out = run(input).await;
        assert!(out.contains(r#""id":4"#));
    }

    #[tokio::test]
    async fn handles_several_frames_in_one_read() {
        let mut input = frame(r#"{"jsonrpc":"2.0","id":1,"method":"a"}"#);
        input.extend(frame(r#"{"jsonrpc":"2.0","id":2,"method":"b"}"#));
        input.extend(frame(r#"{"jsonrpc":"2.0","id":3,"method":"c"}"#));
        let out = run(input).await;
        assert_eq!(out.matches("Content-Length").count(), 3);
    }

    /// Bytes arriving a few at a time must reassemble — the buffer is the only
    /// thing holding a half-read frame together.
    #[tokio::test]
    async fn reassembles_a_frame_split_across_reads() {
        let whole = frame(r#"{"jsonrpc":"2.0","id":9,"method":"initialize"}"#);
        let (mut writer, reader) = tokio::io::duplex(4);
        let feeder = tokio::spawn(async move {
            for byte in whole {
                writer.write_all(&[byte]).await.unwrap();
            }
        });

        let mut out: Vec<u8> = Vec::new();
        sanitize(reader, &mut out).await.unwrap();
        feeder.await.unwrap();

        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(r#""id":9"#), "got {out:?}");
    }

    #[tokio::test]
    async fn recovers_when_the_body_is_shorter_than_advertised() {
        // Truncated stream: the frame never completes, so nothing is forwarded
        // and we exit cleanly rather than hanging or panicking.
        let input = b"Content-Length: 500\r\n\r\n{\"jsonrpc\":\"2.0\"}".to_vec();
        let out = run(input).await;
        assert_eq!(out, "");
    }

    #[test]
    fn oversized_header_block_is_discarded() {
        let mut buf = vec![b'x'; MAX_HEADER_BYTES + 1];
        assert!(matches!(step(&mut buf), Step::Dropped));
        assert!(buf.is_empty());
    }

    #[test]
    fn an_absurd_content_length_is_rejected_without_allocating() {
        let mut buf = b"Content-Length: 999999999999\r\n\r\n".to_vec();
        assert!(matches!(step(&mut buf), Step::Dropped));
        assert!(buf.is_empty());
    }
}
