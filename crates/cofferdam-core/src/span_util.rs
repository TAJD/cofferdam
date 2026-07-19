//! Span conversion helpers — oxc gives byte offsets, we need line/column
//! for human-readable output and editor links.
//!
//! O(N) per call but cheap (typical TS file <100kB, finding count small).
//! When checks grow numerous we'll precompute a line-table per file —
//! that's an engine-level optimization that doesn't affect this API.

use crate::issue::Span;

/// Convert a byte range in `text` into a `Span` (1-based line/column).
///
/// Counts `\n` up to `start_byte` for the line, then bytes since the
/// last `\n` for the column. UTF-8 multi-byte chars in the leading
/// portion of a line will inflate the column count slightly — phase 2
/// will switch this to grapheme-aware once the unicode-segmentation
/// crate is in the tree.
pub fn span_from_bytes(text: &str, start_byte: u32, end_byte: u32) -> Span {
    let start = start_byte as usize;
    let bytes = text.as_bytes();

    let mut line: u32 = 1;
    let mut last_nl: usize = 0;
    for (i, &b) in bytes.iter().enumerate().take(start.min(bytes.len())) {
        if b == b'\n' {
            line += 1;
            last_nl = i + 1;
        }
    }
    let column = (start.saturating_sub(last_nl)) as u32 + 1;

    Span {
        start_byte,
        end_byte,
        line,
        column,
    }
}

/// Precomputed line-start table for resolving many spans in the same file
/// in O(log n) each, instead of paying `span_from_bytes`'s O(start_byte)
/// scan per call. Checks that collect candidate byte ranges across a whole
/// file (e.g. every string literal, every sliding statement window) before
/// knowing which ones will actually be reported should build one `LineIndex`
/// per file and resolve spans only for the findings that survive filtering.
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = Vec::with_capacity(text.bytes().filter(|&b| b == b'\n').count() + 1);
        line_starts.push(0u32);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        Self { line_starts }
    }

    /// Same line/column semantics as [`span_from_bytes`], resolved via
    /// binary search over the precomputed line-start table.
    pub fn span_from_bytes(&self, start_byte: u32, end_byte: u32) -> Span {
        let idx = match self.line_starts.binary_search(&start_byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let last_nl = self.line_starts[idx];
        let line = idx as u32 + 1;
        let column = start_byte.saturating_sub(last_nl) + 1;

        Span {
            start_byte,
            end_byte,
            line,
            column,
        }
    }
}
