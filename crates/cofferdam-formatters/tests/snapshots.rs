//! Snapshot tests for cofferdam-formatters.
//!
//! Lock down the output contract for TextFormatter, JsonFormatter (compact +
//! pretty), and CompactFormatter. The corpus here is deliberately minimal but
//! covers every feature the formatters are expected to render:
//!
//! - At least one issue from each of the five categories.
//! - One issue with `related` spans (cross-file finding, a la
//!   Design.DuplicateExportName).
//! - A mix of priorities so the per-category sort order is exercised.
//! - Multiple severities.
//!
//! If a test fails it means a formatter changed its output — intentional
//! changes should be accepted with `cargo insta accept` (or
//! `INSTA_UPDATE=always cargo test`) and reviewed in the diff before merging.

use cofferdam_core::{Issue, Priority, RelatedSpan, Severity, Span};
use cofferdam_formatters::{CompactFormatter, JsonFormatter, TextFormatter};
use std::path::PathBuf;

// ── fixture helpers ──────────────────────────────────────────────────────────

fn span(line: u32, column: u32, start_byte: u32, end_byte: u32) -> Span {
    Span {
        line,
        column,
        start_byte,
        end_byte,
    }
}

fn related(file: &str, line: u32, column: u32, start_byte: u32, end_byte: u32) -> RelatedSpan {
    RelatedSpan {
        file: PathBuf::from(file),
        span: span(line, column, start_byte, end_byte),
    }
}

/// Build the fixed set of issues used in every snapshot test.
///
/// Five categories, mixed priorities, one cross-file finding with `related`.
fn fixture_issues() -> Vec<Issue> {
    vec![
        // Consistency — lower priority
        Issue {
            check_id: "Consistency.QuoteStyle".into(),
            message: "use single quotes to match the dominant style in this project".into(),
            file: PathBuf::from("src/app.ts"),
            span: span(12, 5, 200, 215),
            priority: Priority::LOW,
            severity: Severity::Low,
            related: Vec::new(),
            fix: None,
        },
        // Design — cross-file finding with `related` spans (mirrors DuplicateExportName shape)
        Issue {
            check_id: "Design.DuplicateExportName".into(),
            message: "export `Button` appears in multiple barrel files".into(),
            file: PathBuf::from("src/components/index.ts"),
            span: span(3, 1, 40, 56),
            priority: Priority::NORMAL,
            severity: Severity::Medium,
            related: vec![
                related("src/ui/index.ts", 7, 1, 120, 136),
                related("src/shared/index.ts", 2, 1, 18, 34),
            ],
            fix: None,
        },
        // Readability — below-normal priority
        Issue {
            check_id: "Readability.MaxLineLength".into(),
            message: "line is 145 characters, exceeds limit of 120".into(),
            file: PathBuf::from("src/utils/format.ts"),
            span: span(88, 121, 3200, 3344),
            priority: Priority(-3),
            severity: Severity::Info,
            related: Vec::new(),
            fix: None,
        },
        // Refactor — higher priority
        Issue {
            check_id: "Refactor.CyclomaticComplexity".into(),
            message: "function `processOrder` has cyclomatic complexity 14, exceeds limit of 10"
                .into(),
            file: PathBuf::from("src/orders/processor.ts"),
            span: span(22, 1, 580, 620),
            priority: Priority::HIGHER,
            severity: Severity::High,
            related: Vec::new(),
            fix: None,
        },
        // Warning — critical severity
        Issue {
            check_id: "Warning.TripleEquals".into(),
            message: "use `===` instead of `==` to avoid type coercion".into(),
            file: PathBuf::from("src/auth/validate.ts"),
            span: span(47, 18, 1402, 1404),
            priority: Priority::HIGHER,
            severity: Severity::Critical,
            related: Vec::new(),
            fix: None,
        },
    ]
}

// ── TextFormatter ────────────────────────────────────────────────────────────

#[test]
fn text_render_contract() {
    let issues = fixture_issues();
    insta::assert_snapshot!(TextFormatter::render(&issues));
}

#[test]
fn text_render_quiet() {
    let issues = fixture_issues();
    insta::assert_snapshot!(TextFormatter::render_with_opts(
        &issues,
        cofferdam_formatters::TextRenderOpts { quiet: true }
    ));
}

#[test]
fn text_render_with_baseline() {
    let issues = fixture_issues();
    // First and last issues are baselined; the rest are new.
    let tagged: Vec<(Issue, bool)> = issues
        .into_iter()
        .enumerate()
        .map(|(i, issue)| (issue, i == 0 || i == 4))
        .collect();
    insta::assert_snapshot!(TextFormatter::render_with_baseline(&tagged));
}

#[test]
fn text_render_empty() {
    insta::assert_snapshot!(TextFormatter::render(&[]));
}

// ── JsonFormatter ─────────────────────────────────────────────────────────────

#[test]
fn json_render_compact_contract() {
    let issues = fixture_issues();
    insta::assert_snapshot!(JsonFormatter::render(&issues));
}

#[test]
fn json_render_pretty_contract() {
    let issues = fixture_issues();
    insta::assert_snapshot!(JsonFormatter::render_pretty(&issues));
}

#[test]
fn json_render_empty() {
    insta::assert_snapshot!(JsonFormatter::render(&[]));
}

#[test]
fn json_render_with_baseline() {
    let issues = fixture_issues();
    let tagged: Vec<(Issue, bool)> = issues
        .into_iter()
        .enumerate()
        .map(|(i, issue)| (issue, i == 0 || i == 4))
        .collect();
    insta::assert_snapshot!(JsonFormatter::render_with_baseline_pretty(&tagged));
}

#[test]
fn json_render_truncated() {
    let issues = fixture_issues();
    insta::assert_snapshot!(JsonFormatter::render_with_opts(
        &issues,
        cofferdam_formatters::JsonRenderOpts {
            pretty: true,
            truncated_from: Some(42),
        }
    ));
}

// ── CompactFormatter ──────────────────────────────────────────────────────────

#[test]
fn compact_render_contract() {
    let issues = fixture_issues();
    insta::assert_snapshot!(CompactFormatter::render(&issues));
}

#[test]
fn compact_render_empty() {
    insta::assert_snapshot!(CompactFormatter::render(&[]));
}
