//! Byte-offset → LSP `Position` (line + UTF-16 character) conversion.
//!
//! LSP positions are line + character, where "character" is a UTF-16
//! code unit count (LSP < 3.17 has no other option, and tower-lsp's
//! types target that encoding). Internally Saule spans are byte offsets
//! into the original UTF-8 source, so we precompute the byte offset of
//! every line start once per document and resolve positions on demand.

use tower_lsp::lsp_types::{Position, Range};

/// Map from byte offsets in a UTF-8 source string to LSP positions.
/// Cheap to build (O(n) one-pass scan) and cheap to query (binary search
/// for the line, then a tiny UTF-16 count on the line slice).
pub struct LineIndex {
    /// Byte offsets of the start of each line. Always begins with `0`.
    line_starts: Vec<usize>,
    /// Total source length in bytes; used to clamp out-of-range offsets.
    len: usize,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut line_starts = Vec::with_capacity(64);
        line_starts.push(0);
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            line_starts,
            len: source.len(),
        }
    }

    /// Convert a byte offset into a `Position`. Out-of-range offsets are
    /// clamped to end-of-source so we never panic on a stale span.
    pub fn position(&self, source: &str, byte: usize) -> Position {
        let byte = byte.min(self.len);
        // Last line whose start <= byte.
        let line_idx = match self.line_starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.line_starts[line_idx];
        // UTF-16 code units between line_start and byte.
        let prefix = &source.as_bytes()[line_start..byte];
        // Safe: line starts are at character boundaries (after `\n`) and
        // `byte` is clamped to source.len().
        let prefix_str = std::str::from_utf8(prefix).unwrap_or("");
        let character: u32 = prefix_str.encode_utf16().count() as u32;
        Position {
            line: line_idx as u32,
            character,
        }
    }

    pub fn range(&self, source: &str, start: usize, end: usize) -> Range {
        Range {
            start: self.position(source, start),
            end: self.position(source, end),
        }
    }
}
