//! JSON / robot-mode formatter.
//!
//! Stable, machine-readable output for AI agents and CI pipelines.
//! Schema priorities, in order:
//!
//! 1. **Stable**: field names and types are part of the contract. Adding
//!    fields is fine; renaming or changing types is a breaking change.
//! 2. **Token-economical**: short field names, no ANSI, no decorative
//!    text, no trailing summary line. The whole document parses with
//!    `JSON.parse`.
//! 3. **Self-describing**: each finding carries everything a tool needs
//!    to render it (category, priority, severity, file path, location,
//!    message, check id). No second lookup against `CheckMeta`.
//!
//! `--robot` is the marketed flag; `--format json` is the underlying
//! switch. Future formats (toon, sarif) plug in here.

use cofferdam_core::{Category, Issue, Severity};
use serde::Serialize;

#[derive(Serialize)]
pub struct RobotReport<'a> {
    pub findings: Vec<RobotFinding<'a>>,
    pub summary: RobotSummary,
}

#[derive(Serialize)]
pub struct RobotFinding<'a> {
    /// Dotted check ID, e.g. `Warning.TripleEquals`. Stable string.
    pub id: &'a str,
    /// Lowercase category name (`"warning"`, `"refactor"`, ...).
    pub category: &'static str,
    /// Computed sort priority (-20..=20). Higher fixes first.
    pub priority: i8,
    /// Configured severity (`"info"`, `"warning"`, `"error"`).
    pub severity: &'static str,
    /// Path as cofferdam saw it. Forward-slash normalized so AI agents
    /// can quote it back as a clickable editor link without OS munging.
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// Byte offsets into the file's text. Useful for span-based fixers.
    pub start_byte: u32,
    pub end_byte: u32,
    pub message: &'a str,
}

#[derive(Serialize)]
pub struct RobotSummary {
    pub total: usize,
    /// Per-category totals, only categories with > 0 findings present.
    /// Stable keys: `consistency`, `design`, `readability`, `refactor`,
    /// `warning`.
    pub by_category: std::collections::BTreeMap<&'static str, usize>,
}

pub struct JsonFormatter;

impl JsonFormatter {
    /// Render findings as compact JSON (one line, no whitespace).
    pub fn render(issues: &[Issue]) -> String {
        Self::render_inner(issues, false)
    }

    /// Render findings as pretty-printed JSON. Use for human inspection.
    pub fn render_pretty(issues: &[Issue]) -> String {
        Self::render_inner(issues, true)
    }

    fn render_inner(issues: &[Issue], pretty: bool) -> String {
        let findings: Vec<RobotFinding<'_>> = issues
            .iter()
            .map(|i| RobotFinding {
                id: i.check_id.as_str(),
                category: category_str(category_of(&i.check_id)),
                priority: i.priority.0,
                severity: severity_str(i.severity),
                file: normalize_path(&i.file),
                line: i.span.line,
                column: i.span.column,
                start_byte: i.span.start_byte,
                end_byte: i.span.end_byte,
                message: i.message.as_str(),
            })
            .collect();

        let mut by_category: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for f in &findings {
            *by_category.entry(f.category).or_insert(0) += 1;
        }

        let report = RobotReport {
            summary: RobotSummary {
                total: findings.len(),
                by_category,
            },
            findings,
        };

        if pretty {
            serde_json::to_string_pretty(&report).expect("RobotReport serializes infallibly")
        } else {
            serde_json::to_string(&report).expect("RobotReport serializes infallibly")
        }
    }
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

fn severity_str(sev: Severity) -> &'static str {
    match sev {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// Forward-slash normalize. Windows native paths use `\`, but agents and
/// editor protocols universally accept `/` and that's what cd-ose wants
/// from the text formatter too.
fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
