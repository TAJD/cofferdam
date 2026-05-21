//! Compact / pipe-delimited formatter.
//!
//! One header line followed by one record per finding. Most token-
//! economical output cofferdam ships — designed for the case where an
//! AI agent shovels a report into a prompt and pays per byte. JSON
//! stays the canonical machine format with full schema fidelity;
//! compact is the agent-context-injection format.
//!
//! ## Schema (stable contract)
//!
//! Header is fixed at exactly eight `|`-separated columns:
//!
//! ```text
//! priority|severity|category|id|file|line|column|message
//! ```
//!
//! Records mirror the header order. The message is the **last** column,
//! so any embedded `|` characters in a message render naked — parsers
//! split on `|` for the first 7 fields and take the rest of the line
//! verbatim as the message. (Rust's `splitn(8, '|')` is the natural
//! shape.) Cofferdam's check messages don't contain newlines today, but
//! we collapse any embedded newlines to spaces defensively so the
//! line-per-finding contract is unconditional.
//!
//! ## Empty result
//!
//! Header line only. Still well-formed and parseable — agents that
//! always read the first line for column names will not crash on a
//! clean run.
//!
//! ## Baseline tags
//!
//! Compact mode v1 does not carry baseline information. Use
//! `--format=json` if you need per-finding `baselined` flags.

use cofferdam_core::{Category, Issue};
use std::fmt::Write;

/// Header line, exactly as emitted (no trailing newline).
pub const COMPACT_HEADER: &str = "priority|severity|category|id|file|line|column|message";

/// Pipe-delimited line-per-finding formatter — the most
/// token-economical output, designed for piping findings into an AI
/// prompt or further tooling. Header line is `COMPACT_HEADER`.
pub struct CompactFormatter;

impl CompactFormatter {
    /// Render every finding as one pipe-delimited line under a fixed
    /// header. Output ends in a newline whether the findings list is
    /// empty or not.
    pub fn render(issues: &[Issue]) -> String {
        let mut out = String::with_capacity(estimate_capacity(issues));
        out.push_str(COMPACT_HEADER);
        out.push('\n');
        for issue in issues {
            write_record(&mut out, issue);
            out.push('\n');
        }
        out
    }
}

/// Pre-size the buffer. ~80 chars per finding is the empirical
/// average; doesn't have to be exact, just avoids growth churn.
fn estimate_capacity(issues: &[Issue]) -> usize {
    COMPACT_HEADER.len() + 1 + issues.len() * 96
}

fn write_record(out: &mut String, issue: &Issue) {
    let category = category_str(category_of(&issue.check_id));
    let _ = write!(
        out,
        "{}|{}|{}|{}|{}|{}|{}|",
        issue.priority.0,
        issue.severity.as_str(),
        category,
        issue.check_id,
        normalize_path(&issue.file),
        issue.span.line,
        issue.span.column,
    );
    write_message(out, &issue.message);
}

/// Collapse newlines to spaces so the line-per-finding contract holds
/// even if a future check emits a multi-line message. We don't escape
/// pipes because the message is the trailing column — naked pipes
/// inside the message are fine for `splitn(8, '|')`-style parsers.
fn write_message(out: &mut String, message: &str) {
    for ch in message.chars() {
        match ch {
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
}

/// Forward-slash normalize. Same convention as the text and JSON
/// formatters — agent-friendly, copy-paste-able as an editor link.
fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn category_of(check_id: &str) -> Option<Category> {
    match check_id.split('.').next()? {
        "Consistency" => Some(Category::Consistency),
        "Design" => Some(Category::Design),
        "Readability" => Some(Category::Readability),
        "Refactor" => Some(Category::Refactor),
        "Warning" => Some(Category::Warning),
        _ => None,
    }
}

fn category_str(cat: Option<Category>) -> &'static str {
    match cat {
        Some(Category::Consistency) => "consistency",
        Some(Category::Design) => "design",
        Some(Category::Readability) => "readability",
        Some(Category::Refactor) => "refactor",
        Some(Category::Warning) => "warning",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Priority, Severity, Span};
    use std::path::PathBuf;

    fn make_issue(file: PathBuf, check_id: &str, message: &str) -> Issue {
        Issue {
            file,
            span: Span {
                line: 8,
                column: 121,
                start_byte: 0,
                end_byte: 10,
            },
            message: message.into(),
            check_id: check_id.into(),
            severity: Severity::Low,
            priority: Priority(-5),
            related: Vec::new(),
        }
    }

    #[test]
    fn empty_output_is_header_only() {
        let out = CompactFormatter::render(&[]);
        assert_eq!(out, format!("{COMPACT_HEADER}\n"));
    }

    #[test]
    fn header_is_first_line_and_stable() {
        let issue = make_issue(
            PathBuf::from("examples/sample.ts"),
            "Readability.MaxLineLength",
            "line is 149 characters, exceeds limit of 120",
        );
        let out = CompactFormatter::render(&[issue]);
        let first_line = out.lines().next().unwrap();
        assert_eq!(
            first_line,
            "priority|severity|category|id|file|line|column|message"
        );
    }

    #[test]
    fn record_round_trips_via_splitn_8() {
        let issue = make_issue(
            PathBuf::from("examples/sample.ts"),
            "Readability.MaxLineLength",
            "line is 149 characters, exceeds limit of 120",
        );
        let out = CompactFormatter::render(&[issue]);
        let record = out.lines().nth(1).expect("record line exists");
        let parts: Vec<&str> = record.splitn(8, '|').collect();
        assert_eq!(parts.len(), 8);
        assert_eq!(parts[0], "-5");
        assert_eq!(parts[1], "low");
        assert_eq!(parts[2], "readability");
        assert_eq!(parts[3], "Readability.MaxLineLength");
        assert_eq!(parts[4], "examples/sample.ts");
        assert_eq!(parts[5], "8");
        assert_eq!(parts[6], "121");
        assert_eq!(parts[7], "line is 149 characters, exceeds limit of 120");
    }

    #[test]
    fn message_with_pipe_renders_naked_in_last_column() {
        let issue = make_issue(
            PathBuf::from("a.ts"),
            "Warning.Test",
            "got a|b|c — three pipes",
        );
        let out = CompactFormatter::render(&[issue]);
        let record = out.lines().nth(1).unwrap();
        let parts: Vec<&str> = record.splitn(8, '|').collect();
        assert_eq!(parts[7], "got a|b|c — three pipes");
    }

    #[test]
    fn message_newlines_collapse_to_space() {
        let issue = make_issue(
            PathBuf::from("a.ts"),
            "Warning.Test",
            "first line\nsecond line\rthird line",
        );
        let out = CompactFormatter::render(&[issue]);
        // Total lines: header + 1 record. If newlines weren't collapsed,
        // we'd see additional lines.
        assert_eq!(out.lines().count(), 2);
        let record = out.lines().nth(1).unwrap();
        let parts: Vec<&str> = record.splitn(8, '|').collect();
        assert_eq!(parts[7], "first line second line third line");
    }

    #[test]
    fn windows_paths_are_forward_slashed() {
        let issue = make_issue(
            PathBuf::from(r"C:\Users\demo\src\foo.ts"),
            "Warning.Test",
            "msg",
        );
        let out = CompactFormatter::render(&[issue]);
        assert!(out.contains("C:/Users/demo/src/foo.ts"));
        assert!(!out.contains(r"\Users"));
    }

    #[test]
    fn unknown_category_renders_as_unknown() {
        let issue = make_issue(PathBuf::from("a.ts"), "Bogus.Check", "msg");
        let out = CompactFormatter::render(&[issue]);
        let record = out.lines().nth(1).unwrap();
        let parts: Vec<&str> = record.splitn(8, '|').collect();
        assert_eq!(parts[2], "unknown");
    }
}
