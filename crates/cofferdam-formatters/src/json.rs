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

use cofferdam_core::{Category, Issue, RelatedSpan, Severity};
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
    /// Other locations participating in the same finding (e.g. duplicate
    /// blocks). Omitted entirely when the finding is single-location.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedFinding>,
    /// True when this finding matched an entry in the active baseline
    /// (so it should not fail CI under `--fail-on-new`). Omitted when no
    /// baseline is active so the schema for the no-baseline case stays
    /// byte-identical to pre-baseline cofferdam.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baselined: Option<bool>,
}

#[derive(Serialize)]
pub struct RelatedFinding {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[derive(Serialize)]
pub struct RobotSummary {
    pub total: usize,
    /// Per-category totals, only categories with > 0 findings present.
    /// Stable keys: `consistency`, `design`, `readability`, `refactor`,
    /// `warning`.
    pub by_category: std::collections::BTreeMap<&'static str, usize>,
    /// Count of findings not matching the active baseline. Omitted when
    /// no baseline is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<usize>,
    /// Count of findings matching the active baseline. Omitted when no
    /// baseline is active.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baselined: Option<usize>,
    /// Total findings produced before `--max-issues` truncation. Omitted
    /// when no truncation occurred so byte-for-byte JSON output is
    /// identical to pre-cd-y7e cofferdam in the common case. When
    /// present, it is always strictly greater than `total` (which
    /// reflects the rendered `findings` array length).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_from: Option<usize>,
}

/// Options for the JSON formatter. `pretty` is in here so callers don't
/// have to pick between `_pretty` and `_with_opts` method variants.
#[derive(Copy, Clone, Debug, Default)]
pub struct JsonRenderOpts {
    pub pretty: bool,
    /// Original total before `--max-issues` truncation, when truncation
    /// happened. `None` when the rendered findings are the complete set.
    pub truncated_from: Option<usize>,
}

pub struct JsonFormatter;

impl JsonFormatter {
    /// Render findings as compact JSON (one line, no whitespace).
    pub fn render(issues: &[Issue]) -> String {
        Self::render_with_opts(issues, JsonRenderOpts::default())
    }

    /// Render findings as pretty-printed JSON. Use for human inspection.
    pub fn render_pretty(issues: &[Issue]) -> String {
        Self::render_with_opts(
            issues,
            JsonRenderOpts {
                pretty: true,
                truncated_from: None,
            },
        )
    }

    pub fn render_with_opts(issues: &[Issue], opts: JsonRenderOpts) -> String {
        Self::render_inner(issues, opts)
    }

    fn render_inner(issues: &[Issue], opts: JsonRenderOpts) -> String {
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
                related: i.related.iter().map(map_related).collect(),
                baselined: None,
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
                new: None,
                baselined: None,
                truncated_from: opts.truncated_from,
            },
            findings,
        };

        if opts.pretty {
            serde_json::to_string_pretty(&report).expect("RobotReport serializes infallibly")
        } else {
            serde_json::to_string(&report).expect("RobotReport serializes infallibly")
        }
    }

    /// Render with per-finding baseline tags. `tagged` is parallel to the
    /// engine's output: each `(Issue, bool)` pair carries `true` when the
    /// finding matched the active baseline. Adds `baselined` per finding
    /// and `new` / `baselined` totals on the summary.
    pub fn render_with_baseline(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_with_opts(tagged, JsonRenderOpts::default())
    }

    /// Pretty-printed variant of `render_with_baseline`.
    pub fn render_with_baseline_pretty(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_with_opts(
            tagged,
            JsonRenderOpts {
                pretty: true,
                truncated_from: None,
            },
        )
    }

    pub fn render_with_baseline_with_opts(
        tagged: &[(Issue, bool)],
        opts: JsonRenderOpts,
    ) -> String {
        Self::render_with_baseline_inner(tagged, opts)
    }

    fn render_with_baseline_inner(tagged: &[(Issue, bool)], opts: JsonRenderOpts) -> String {
        let findings: Vec<RobotFinding<'_>> = tagged
            .iter()
            .map(|(i, baselined)| RobotFinding {
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
                related: i.related.iter().map(map_related).collect(),
                baselined: Some(*baselined),
            })
            .collect();

        let mut by_category: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for f in &findings {
            *by_category.entry(f.category).or_insert(0) += 1;
        }
        let baselined_count = findings
            .iter()
            .filter(|f| f.baselined == Some(true))
            .count();
        let new_count = findings.len() - baselined_count;

        let report = RobotReport {
            summary: RobotSummary {
                total: findings.len(),
                by_category,
                new: Some(new_count),
                baselined: Some(baselined_count),
                truncated_from: opts.truncated_from,
            },
            findings,
        };

        if opts.pretty {
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
    sev.as_str()
}

/// Forward-slash normalize. Windows native paths use `\`, but agents and
/// editor protocols universally accept `/` and that's what cd-ose wants
/// from the text formatter too.
fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn map_related(r: &RelatedSpan) -> RelatedFinding {
    RelatedFinding {
        file: normalize_path(&r.file),
        line: r.span.line,
        column: r.span.column,
        start_byte: r.span.start_byte,
        end_byte: r.span.end_byte,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Priority, Severity, Span};
    use std::path::PathBuf;

    fn make_issue(file: PathBuf, check_id: &str) -> Issue {
        Issue {
            file,
            span: Span {
                line: 1,
                column: 5,
                start_byte: 0,
                end_byte: 10,
            },
            message: "test message".into(),
            check_id: check_id.into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        }
    }

    #[test]
    fn json_formatter_normalizes_windows_paths() {
        let issue = make_issue(PathBuf::from(r"C:\Users\demo\src\foo.ts"), "Warning.Test");
        let output = JsonFormatter::render(&[issue]);
        assert!(output.contains("C:/Users/demo/src/foo.ts"));
        assert!(!output.contains(r"\Users"));
    }

    #[test]
    fn json_formatter_preserves_forward_slash_paths() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render(&[issue]);
        assert!(output.contains("src/foo.ts"));
    }

    #[test]
    fn json_formatter_empty_findings() {
        let output = JsonFormatter::render(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["findings"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["summary"]["total"], 0);
    }

    #[test]
    fn json_formatter_omits_truncated_from_when_not_truncated() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render(&[issue]);
        assert!(
            !output.contains("truncated_from"),
            "schema must stay byte-identical when no truncation, got:\n{output}"
        );
    }

    #[test]
    fn json_formatter_includes_truncated_from_when_capped() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render_with_opts(
            &[issue],
            JsonRenderOpts {
                pretty: false,
                truncated_from: Some(42),
            },
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["summary"]["truncated_from"], 42);
        assert_eq!(parsed["summary"]["total"], 1);
    }

    #[test]
    fn json_formatter_baseline_includes_truncated_from() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render_with_baseline_with_opts(
            &[(issue, false)],
            JsonRenderOpts {
                pretty: false,
                truncated_from: Some(99),
            },
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["summary"]["truncated_from"], 99);
    }
}
