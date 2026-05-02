//! Integration tests for `cofferdam-core::span_util` module.
//!
//! Acceptance criteria:
//! - ASCII text produces correct 1-based line/column
//! - UTF-8 multibyte sequences handled correctly
//! - CRLF input stripped correctly but doesn't break line counts
//! - Positions at end-of-file handled
//! - Empty files don't panic

use cofferdam_core::span_from_bytes;

// ============================================================
// ASCII text — straightforward line/column tracking
// ============================================================

#[test]
fn span_from_bytes_single_line_start() {
    let text = "hello world";
    let span = span_from_bytes(text, 0, 5);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 1);
    assert_eq!(span.start_byte, 0);
    assert_eq!(span.end_byte, 5);
}

#[test]
fn span_from_bytes_single_line_middle() {
    let text = "hello world";
    let span = span_from_bytes(text, 6, 11);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 7);
}

#[test]
fn span_from_bytes_second_line_start() {
    let text = "hello\nworld";
    let span = span_from_bytes(text, 6, 11);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_second_line_middle() {
    let text = "hello\nworld";
    let span = span_from_bytes(text, 8, 11);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 3);
}

#[test]
fn span_from_bytes_multiple_newlines() {
    let text = "a\nb\nc\nd";
    // 'd' is at byte offset 6
    let span = span_from_bytes(text, 6, 7);

    assert_eq!(span.line, 4);
    assert_eq!(span.column, 1);
}

// ============================================================
// UTF-8 multibyte sequences
// ============================================================

#[test]
fn span_from_bytes_utf8_multibyte_same_line() {
    let text = "hello 🦀 world";
    // Position after emoji (which is 4 bytes in UTF-8)
    let emoji_start = 6u32;
    let span = span_from_bytes(text, emoji_start, emoji_start + 4);

    assert_eq!(span.line, 1);
    // Column is byte-position, not grapheme count
    assert_eq!(span.column, emoji_start + 1);
}

#[test]
fn span_from_bytes_utf8_emoji_before_newline() {
    let text = "hello 🦀\nworld";
    let span = span_from_bytes(text, 0, 8);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_utf8_on_second_line() {
    let text = "line1\n🦀 test";
    // After newline
    let span = span_from_bytes(text, 6, 10);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_mixed_ascii_and_unicode() {
    let text = "café";
    // é is 2 bytes
    let span = span_from_bytes(text, 0, 4);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 1);
}

// ============================================================
// CRLF handling
// ============================================================

#[test]
fn span_from_bytes_crlf_single_line() {
    let text = "hello\r\nworld";
    // \r is byte 5, \n is byte 6, 'w' is byte 7
    let span = span_from_bytes(text, 7, 12);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_multiple_crlf() {
    let text = "a\r\nb\r\nc";
    // 'c' is at byte 8 (a=0, \r=1, \n=2, b=3, \r=4, \n=5, c=6)
    // Actually 'c' is at byte 6
    let span = span_from_bytes(text, 6, 7);

    assert_eq!(span.line, 3);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_mixed_lf_and_crlf() {
    // Real-world scenario: inconsistent line endings
    let text = "a\r\nb\nc";
    // a=0, \r=1, \n=2, b=3, \n=4, c=5
    // 'c' is at byte 5
    let span = span_from_bytes(text, 5, 6);

    assert_eq!(span.line, 3);
    assert_eq!(span.column, 1);
}

// ============================================================
// End-of-file positions
// ============================================================

#[test]
fn span_from_bytes_at_end_of_file() {
    let text = "hello";
    let span = span_from_bytes(text, 5, 5);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 6);
}

#[test]
fn span_from_bytes_after_final_newline() {
    let text = "hello\n";
    let span = span_from_bytes(text, 6, 6);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_multiple_lines_ending_at_eof() {
    let text = "a\nb\nc";
    let span = span_from_bytes(text, 4, 5);

    assert_eq!(span.line, 3);
    assert_eq!(span.column, 1);
}

// ============================================================
// Empty and degenerate cases
// ============================================================

#[test]
fn span_from_bytes_empty_file() {
    let text = "";
    let span = span_from_bytes(text, 0, 0);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_single_character() {
    let text = "x";
    let span = span_from_bytes(text, 0, 1);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_single_newline() {
    let text = "\n";
    let span = span_from_bytes(text, 1, 1);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

// ============================================================
// End byte tracking (independent of line/column)
// ============================================================

#[test]
fn span_from_bytes_preserves_byte_range() {
    let text = "hello world";
    let span = span_from_bytes(text, 3, 8);

    assert_eq!(span.start_byte, 3);
    assert_eq!(span.end_byte, 8);
}

#[test]
fn span_from_bytes_zero_width_span() {
    let text = "hello";
    let span = span_from_bytes(text, 2, 2);

    assert_eq!(span.start_byte, 2);
    assert_eq!(span.end_byte, 2);
    assert_eq!(span.line, 1);
    assert_eq!(span.column, 3);
}

// ============================================================
// Real-world scenario: TypeScript code snippets
// ============================================================

#[test]
fn span_from_bytes_typescript_function() {
    let text = "function foo() {\n  return 42;\n}";
    // Start of 'return' keyword on line 2
    let return_pos = text.find("return").unwrap() as u32;
    let span = span_from_bytes(text, return_pos, return_pos + 6);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 3);
}

#[test]
fn span_from_bytes_multiline_string() {
    let text = "const s = `line1\nline2\nline3`;";
    // Position in middle of template literal (line 2)
    let span = span_from_bytes(text, 18, 23);

    assert_eq!(span.line, 2);
}

#[test]
fn span_from_bytes_after_block_comment() {
    let text = "/* comment */\nconst x = 1;";
    // Position of 'const'
    let const_pos = text.find("const").unwrap() as u32;
    let span = span_from_bytes(text, const_pos, const_pos + 5);

    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn span_from_bytes_unicode_identifier() {
    let text = "const café = 1;";
    // é is 2 bytes, so 'c' starts at 6, 'a' at 7, 'f' at 8, 'é' at 9
    let span = span_from_bytes(text, 6, 11);

    assert_eq!(span.line, 1);
    assert_eq!(span.column, 7);
}
