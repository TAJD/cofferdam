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

use cofferdam_core::{Issue, Location, Priority, RelatedSpan, Severity, Span};
use cofferdam_formatters::{CompactFormatter, JsonFormatter, SarifFormatter, TextFormatter};
use std::path::{Path, PathBuf};

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
    let path = PathBuf::from(file);
    RelatedSpan {
        location: Location::from_span(Path::new(file), span(line, column, start_byte, end_byte)),
        file: path,
    }
}

/// Build the fixed set of issues used in every snapshot test.
///
/// Five categories, mixed priorities, one cross-file finding with `related`.
fn fixture_issues() -> Vec<Issue> {
    let app_ts = PathBuf::from("src/app.ts");
    let components_ts = PathBuf::from("src/components/index.ts");
    let utils_ts = PathBuf::from("src/utils/format.ts");
    let orders_ts = PathBuf::from("src/orders/processor.ts");
    let auth_ts = PathBuf::from("src/auth/validate.ts");
    vec![
        // Consistency — lower priority
        Issue {
            check_id: "Consistency.QuoteStyle".into(),
            message: "use single quotes to match the dominant style in this project".into(),
            location: Location::from_span(&app_ts, span(12, 5, 200, 215)),
            file: app_ts,
            priority: Priority::LOW,
            severity: Severity::Low,
            related: Vec::new(),
        },
        // Design — cross-file finding with `related` spans (mirrors DuplicateExportName shape)
        Issue {
            check_id: "Design.DuplicateExportName".into(),
            message: "export `Button` appears in multiple barrel files".into(),
            location: Location::from_span(&components_ts, span(3, 1, 40, 56)),
            file: components_ts,
            priority: Priority::NORMAL,
            severity: Severity::Medium,
            related: vec![
                related("src/ui/index.ts", 7, 1, 120, 136),
                related("src/shared/index.ts", 2, 1, 18, 34),
            ],
        },
        // Readability — below-normal priority
        Issue {
            check_id: "Readability.MaxLineLength".into(),
            message: "line is 145 columns, exceeds limit of 120".into(),
            location: Location::from_span(&utils_ts, span(88, 121, 3200, 3344)),
            file: utils_ts,
            priority: Priority(-3),
            severity: Severity::Info,
            related: Vec::new(),
        },
        // Refactor — higher priority
        Issue {
            check_id: "Refactor.CyclomaticComplexity".into(),
            message: "function `processOrder` has cyclomatic complexity 14, exceeds limit of 10"
                .into(),
            location: Location::from_span(&orders_ts, span(22, 1, 580, 620)),
            file: orders_ts,
            priority: Priority::HIGHER,
            severity: Severity::High,
            related: Vec::new(),
        },
        // Warning — critical severity
        Issue {
            check_id: "Warning.TripleEquals".into(),
            message: "use `===` instead of `==` to avoid type coercion".into(),
            location: Location::from_span(&auth_ts, span(47, 18, 1402, 1404)),
            file: auth_ts,
            priority: Priority::HIGHER,
            severity: Severity::Critical,
            related: Vec::new(),
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
        cofferdam_formatters::TextRenderOpts {
            quiet: true,
            ..Default::default()
        }
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
        &[],
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

// ── SarifFormatter ────────────────────────────────────────────────────────────
//
// Two non-deterministic values are redacted so snapshots are stable across
// Rust versions and machines:
//   * tool version  — changes on every release; replaced with "[VERSION]"
//   * fingerprints  — DefaultHasher seed varies; replaced with "[FINGERPRINT]"

#[test]
fn sarif_render_contract() {
    let issues = fixture_issues();
    // Pass &[] for metas → synthesised stub rules (one per unique check id).
    // This keeps the test self-contained (no cofferdam-checks dep) while still
    // exercising the full result/relatedLocations/partialFingerprints output.
    insta::with_settings!({
        filters => vec![
            (env!("CARGO_PKG_VERSION"), "[VERSION]"),
            (r#""cofferdam/v1": "[0-9a-f]{16}""#, r#""cofferdam/v1": "[FINGERPRINT]""#),
        ]
    }, {
        insta::assert_snapshot!(SarifFormatter::render_pretty(&issues, &[]));
    });
}

#[test]
fn sarif_render_empty() {
    insta::with_settings!({
        filters => vec![
            (env!("CARGO_PKG_VERSION"), "[VERSION]"),
        ]
    }, {
        insta::assert_snapshot!(SarifFormatter::render_pretty(&[], &[]));
    });
}
