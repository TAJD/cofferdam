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
