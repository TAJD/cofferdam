//! Property-based tests for `span_from_bytes`.
//!
//! We test three invariants:
//!
//! 1. **Round-trip**: `span_from_bytes(text, start, end)` produces `(line, column)` that
//!    round-trip back to `start_byte` via the inverse computation (walk lines counting
//!    bytes, then offset from line-start).
//!
//! 2. **Byte passthrough**: `span.start_byte == start` and `span.end_byte == end`.
//!
//! 3. **1-indexed invariants**: `line >= 1` and `column >= 1` for all valid inputs.
//!
//! NOTE on column semantics: `span_from_bytes` uses **byte** column (bytes since the last
//! `\n`), NOT Unicode char column. The source comment says "phase 2 will switch to
//! grapheme-aware"; for now the inverse just replays the same byte arithmetic.

use cofferdam_core::span_from_bytes;
use proptest::prelude::*;

// ──────────────────────────────────────────────────────────────────────────
// Inverse helper (test-only)
// ──────────────────────────────────────────────────────────────────────────

/// Reconstruct `start_byte` from `(line, column)` produced by `span_from_bytes`.
///
/// Mirrors the production algorithm exactly:
/// - Walk bytes, counting `\n` until `line - 1` newlines have been seen.
/// - The byte immediately after the last `\n` is the line start.
/// - `start_byte = line_start + (column - 1)`.
///
/// Returns `None` if `(line, column)` would place the byte past the end of
/// `text` (should not happen under correct production code).
fn inverse_span(text: &str, line: u32, column: u32) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut current_line: u32 = 1;
    let mut line_start: usize = 0;

    for (i, &b) in bytes.iter().enumerate() {
        if current_line == line {
            break;
        }
        if b == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }

    if current_line < line {
        // Requested line never reached (text shorter than expected)
        return None;
    }

    let byte_offset = line_start + (column as usize) - 1;
    if byte_offset <= bytes.len() {
        Some(byte_offset)
    } else {
        None
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Strategy: generate (text, start, end) with start <= end <= text.len()
// ──────────────────────────────────────────────────────────────────────────

/// Strategy that yields `(text, start_byte, end_byte)` where:
/// - `text` is arbitrary valid UTF-8 (may contain newlines, multi-byte chars)
/// - `0 <= start_byte <= end_byte <= text.len()`
///
/// We bias the text toward multi-byte characters (CJK / emoji) by interleaving
/// them with ASCII so that the byte-vs-char column distinction is exercised.
fn text_and_byte_range() -> impl Strategy<Value = (String, u32, u32)> {
    // ".*" matches any Unicode scalar values including newlines and multi-byte
    // sequences, giving us both CJK (3-byte) and emoji (4-byte) coverage.
    ".*".prop_flat_map(|text: String| {
        let len = text.len() as u32;
        // Pick start in [0, len] then end in [start, len].
        (Just(text.clone()), 0u32..=len).prop_flat_map(move |(t, start)| {
            let end_range = start..=(t.len() as u32);
            (Just(t), Just(start), end_range)
        })
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Property tests
// ──────────────────────────────────────────────────────────────────────────

proptest! {
    /// P1 – Round-trip: (line, column) -> back to start_byte.
    ///
    /// For any valid UTF-8 text and byte offsets, the `(line, column)` produced
    /// by `span_from_bytes` must round-trip back to `start_byte` through the
    /// inverse computation.
    #[test]
    fn prop_span_roundtrip(
        (text, start, end) in text_and_byte_range()
    ) {
        let span = span_from_bytes(&text, start, end);
        let recovered = inverse_span(&text, span.line, span.column);
        prop_assert!(
            recovered == Some(start as usize),
            "round-trip failed: text={:?} start={} end={} -> line={} col={} -> recovered={:?}",
            text, start, end, span.line, span.column, recovered
        );
    }

    /// P2 – Byte passthrough: start_byte and end_byte are stored verbatim.
    #[test]
    fn prop_span_byte_passthrough(
        (text, start, end) in text_and_byte_range()
    ) {
        let span = span_from_bytes(&text, start, end);
        prop_assert_eq!(span.start_byte, start,
            "start_byte mismatch: text={:?} start={} end={}", text, start, end);
        prop_assert_eq!(span.end_byte, end,
            "end_byte mismatch: text={:?} start={} end={}", text, start, end);
    }

    /// P3 – 1-indexed invariants: line >= 1 and column >= 1 always.
    #[test]
    fn prop_span_one_indexed(
        (text, start, end) in text_and_byte_range()
    ) {
        let span = span_from_bytes(&text, start, end);
        prop_assert!(
            span.line >= 1,
            "line should be >= 1, got {} (text={:?} start={} end={})",
            span.line, text, start, end
        );
        prop_assert!(
            span.column >= 1,
            "column should be >= 1, got {} (text={:?} start={} end={})",
            span.column, text, start, end
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Spot-checks (deterministic, validate the inverse helper itself)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn inverse_helper_ascii() {
    // "hello\nworld" — 'w' is at byte 6, line 2, column 1
    assert_eq!(inverse_span("hello\nworld", 2, 1), Some(6));
}

#[test]
fn inverse_helper_multibyte() {
    // "🦀\ntest" — emoji is 4 bytes; 't' starts at byte 5, line 2, column 1
    assert_eq!(inverse_span("🦀\ntest", 2, 1), Some(5));
}

#[test]
fn inverse_helper_line1() {
    // Line 1, column 1 is always byte 0
    assert_eq!(inverse_span("hello", 1, 1), Some(0));
}
