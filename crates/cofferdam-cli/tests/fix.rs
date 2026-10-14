//! Integration tests for `cofferdam fix` — autofix round-trip.
//!
//! CD-357 removed `Warning.TripleEquals`, which was the only built-in
//! check with a real `autofix` implementation. Until a new autofix-capable
//! check lands, this file only exercises the check-independent apply-loop
//! infra (span validation, reverse-offset application) rather than a real
//! `cofferdam fix` round-trip against a builtin.

use std::cmp::Reverse;

use cofferdam_core::{Span, TextEdit};

/// Mirrors the apply loop's span-validity guard in `run_fix` (main.rs):
/// an edit with an invalid byte span (here, `start > end`) is skipped
/// rather than applied, and the skip is counted separately from `applied`.
#[test]
fn apply_edits_skips_invalid_spans_and_counts_them() {
    let original_text = "const a = x == 5;\n".to_string();

    let valid_edit = TextEdit {
        span: Span {
            start_byte: 12,
            end_byte: 14,
            line: 1,
            column: 13,
        },
        replacement: "===".to_string(),
    };
    // Deliberately invalid: start > end.
    let invalid_edit = TextEdit {
        span: Span {
            start_byte: 14,
            end_byte: 12,
            line: 1,
            column: 13,
        },
        replacement: "===".to_string(),
    };

    let mut sorted_edits = vec![valid_edit, invalid_edit];
    sorted_edits.sort_by_key(|e| Reverse(e.span.start_byte));

    let mut text = original_text.clone();
    let mut applied = 0usize;
    let mut skipped = 0usize;
    for edit in &sorted_edits {
        let start = edit.span.start_byte as usize;
        let end = edit.span.end_byte as usize;
        if start > text.len() || end > text.len() || start > end {
            skipped += 1;
            continue;
        }
        text.replace_range(start..end, &edit.replacement);
        applied += 1;
    }

    assert_eq!(applied, 1, "expected exactly one valid edit applied");
    assert_eq!(skipped, 1, "expected exactly one invalid edit skipped");
    assert_eq!(text, "const a = x === 5;\n");
}
