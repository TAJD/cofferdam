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
//! switch. Sibling formatters (`compact`, `sarif`) live next door and
//! share the same `Issue` input.

use std::collections::HashSet;

use cofferdam_core::{docs_url, CheckMeta, Issue, RelatedSpan, Severity};
use serde::Serialize;

use crate::common::{category_of, category_str, normalize_path};

#[derive(Serialize)]
pub(crate) struct RobotReport<'a> {
    pub findings: Vec<RobotFinding<'a>>,
    pub summary: RobotSummary,
}

#[derive(Serialize)]
pub(crate) struct RobotFinding<'a> {
    /// Dotted check ID, e.g. `Warning.TripleEquals`. Stable string.
    pub id: &'a str,
    /// Lowercase category name (`"warning"`, `"refactor"`, ...).
    pub category: &'static str,
    /// Link to the docs-catalog page for this check. Omitted for plugin
    /// checks that have no hosted catalog page (to avoid emitting 404 URLs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// Computed sort priority (-20..=20). Higher fixes first.
    pub priority: i8,
    /// Configured severity (`"info"`, `"warning"`, `"error"`).
    pub severity: &'static str,
    /// Path as cofferdam saw it. Forward-slash normalized so AI agents
    /// can quote it back as a clickable editor link without OS munging.
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// 1-based end line/column, present only for locations that carry a
    /// distinct end position (`LineCol` ranges). Omitted for `Bytes`
    /// (whose end is captured by `end_byte`) and `Custom`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_col: Option<u32>,
    /// Byte offsets into the file's text. Omitted (not zeroed) for
    /// locations with no byte representation (`LineCol` / `Custom`) so
    /// consumers can't mistake "no data" for "offset 0".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_byte: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_byte: Option<u32>,
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
pub(crate) struct RelatedFinding {
    pub file: String,
    pub line: u32,
    pub column: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_byte: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_byte: Option<u32>,
}

#[derive(Serialize)]
pub(crate) struct RobotSummary {
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

/// JSON formatter — emits cofferdam findings as a machine-readable
/// JSON document with a stable schema (the wire structs themselves
/// are `pub(crate)`). Construct via the unit type and call `render` /
/// `render_with_opts` / `render_with_baseline_opts`.
pub struct JsonFormatter;

impl JsonFormatter {
    /// Render findings as compact JSON (one line, no whitespace).
    ///
    /// No builtin-metas are provided; `docs_url` is omitted from all findings.
    /// Use `render_with_opts` with the builtin metas to emit URLs for built-in checks.
    pub fn render(issues: &[Issue]) -> String {
        Self::render_with_opts(issues, &[], JsonRenderOpts::default())
    }

    /// Render findings as pretty-printed JSON. Use for human inspection.
    ///
    /// No builtin-metas are provided; `docs_url` is omitted from all findings.
    pub fn render_pretty(issues: &[Issue]) -> String {
        Self::render_with_opts(
            issues,
            &[],
            JsonRenderOpts {
                pretty: true,
                truncated_from: None,
            },
        )
    }

    /// Render findings honouring the supplied `JsonRenderOpts`
    /// (compact vs pretty, truncation metadata). The JSON schema is
    /// part of the stable surface — additive changes only.
    ///
    /// `metas` is the set of registered builtin check metas. Only findings
    /// whose `check_id` matches an entry in `metas` receive a `docs_url`;
    /// plugin checks (no matching meta) omit the field so callers never see
    /// a URL that 404s.
    pub fn render_with_opts(issues: &[Issue], metas: &[CheckMeta], opts: JsonRenderOpts) -> String {
        let builtin_ids: HashSet<&str> = metas.iter().map(|m| m.id).collect();
        Self::render_inner(issues, opts, &builtin_ids)
    }

    fn render_inner(issues: &[Issue], opts: JsonRenderOpts, builtin_ids: &HashSet<&str>) -> String {
        let findings: Vec<RobotFinding<'_>> = issues
            .iter()
            .map(|i| RobotFinding {
                id: i.check_id.as_str(),
                category: category_str(category_of(&i.check_id)),
                docs_url: builtin_ids
                    .contains(i.check_id.as_str())
                    .then(|| docs_url(i.check_id.as_str())),
                priority: i.priority.0,
                severity: severity_str(i.severity),
                file: normalize_path(&i.file),
                line: i.location.line(),
                column: i.location.column(),
                end_line: i.location.end_line_col().map(|(l, _)| l),
                end_col: i.location.end_line_col().map(|(_, c)| c),
                start_byte: i.location.byte_range().map(|(s, _)| s),
                end_byte: i.location.byte_range().map(|(_, e)| e),
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
    ///
    /// No builtin-metas are provided; `docs_url` is omitted from all findings.
    pub fn render_with_baseline(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_with_opts(tagged, &[], JsonRenderOpts::default())
    }

    /// Pretty-printed variant of `render_with_baseline`.
    ///
    /// No builtin-metas are provided; `docs_url` is omitted from all findings.
    pub fn render_with_baseline_pretty(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_with_opts(
            tagged,
            &[],
            JsonRenderOpts {
                pretty: true,
                truncated_from: None,
            },
        )
    }

    /// Render with per-finding baseline tags and explicit options.
    /// Each finding gains a `baselined: bool` field; the document
    /// gains `summary.new` and `summary.baselined` counts so CI can
    /// gate on the new total alone.
    ///
    /// `metas` is the set of registered builtin check metas. Only findings
    /// whose `check_id` matches an entry in `metas` receive a `docs_url`.
    pub fn render_with_baseline_with_opts(
        tagged: &[(Issue, bool)],
        metas: &[CheckMeta],
        opts: JsonRenderOpts,
    ) -> String {
        let builtin_ids: HashSet<&str> = metas.iter().map(|m| m.id).collect();
        Self::render_with_baseline_inner(tagged, opts, &builtin_ids)
    }

    fn render_with_baseline_inner(
        tagged: &[(Issue, bool)],
        opts: JsonRenderOpts,
        builtin_ids: &HashSet<&str>,
    ) -> String {
        let findings: Vec<RobotFinding<'_>> = tagged
            .iter()
            .map(|(i, baselined)| RobotFinding {
                id: i.check_id.as_str(),
                category: category_str(category_of(&i.check_id)),
                docs_url: builtin_ids
                    .contains(i.check_id.as_str())
                    .then(|| docs_url(i.check_id.as_str())),
                priority: i.priority.0,
                severity: severity_str(i.severity),
                file: normalize_path(&i.file),
                line: i.location.line(),
                column: i.location.column(),
                end_line: i.location.end_line_col().map(|(l, _)| l),
                end_col: i.location.end_line_col().map(|(_, c)| c),
                start_byte: i.location.byte_range().map(|(s, _)| s),
                end_byte: i.location.byte_range().map(|(_, e)| e),
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

fn severity_str(sev: Severity) -> &'static str {
    sev.as_str()
}

fn map_related(r: &RelatedSpan) -> RelatedFinding {
    RelatedFinding {
        file: normalize_path(&r.file),
        line: r.location.line(),
        column: r.location.column(),
        end_line: r.location.end_line_col().map(|(l, _)| l),
        end_col: r.location.end_line_col().map(|(_, c)| c),
        start_byte: r.location.byte_range().map(|(s, _)| s),
        end_byte: r.location.byte_range().map(|(_, e)| e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Location, Priority, Severity, Span};
    use std::path::PathBuf;

    fn make_issue(file: PathBuf, check_id: &str) -> Issue {
        Issue {
            location: Location::from_span(
                &file,
                Span {
                    line: 1,
                    column: 5,
                    start_byte: 0,
                    end_byte: 10,
                },
            ),
            file,
            message: "test message".into(),
            check_id: check_id.into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        }
    }

    // Minimal CheckMeta for testing — only the id field matters for docs_url routing.
    const TRIPLE_EQUALS_META: cofferdam_core::CheckMeta = cofferdam_core::CheckMeta {
        id: "Warning.TripleEquals",
        category: cofferdam_core::Category::Warning,
        base_priority: 15,
        default_severity: cofferdam_core::Severity::High,
        explanation: "",
        body: "",
        requires_types: false,
        consistency: false,
        options: &[],
        autofix: false,
        pure_run: true,
    };
    const CYCLOMATIC_META: cofferdam_core::CheckMeta = cofferdam_core::CheckMeta {
        id: "Refactor.CyclomaticComplexity",
        category: cofferdam_core::Category::Refactor,
        base_priority: 10,
        default_severity: cofferdam_core::Severity::High,
        explanation: "",
        body: "",
        requires_types: false,
        consistency: false,
        options: &[],
        autofix: false,
        pure_run: true,
    };

    #[test]
    fn json_builtin_finding_has_docs_url() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.TripleEquals");
        let output = JsonFormatter::render_with_opts(
            &[issue],
            &[TRIPLE_EQUALS_META],
            JsonRenderOpts::default(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(
            parsed["findings"][0]["docs_url"],
            "https://tajd.github.io/cofferdam/checks/Warning.TripleEquals"
        );
    }

    #[test]
    fn json_plugin_finding_has_no_docs_url() {
        // A plugin check id not in the metas set must not emit a docs_url
        // (the generated URL would 404 — there is no hosted catalog page).
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.TenantIsolation");
        let output = JsonFormatter::render_with_opts(
            &[issue],
            &[TRIPLE_EQUALS_META],
            JsonRenderOpts::default(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(
            parsed["findings"][0]["docs_url"].is_null(),
            "plugin checks must not receive a docs_url"
        );
    }

    #[test]
    fn json_baseline_builtin_finding_has_docs_url() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Refactor.CyclomaticComplexity");
        let output = JsonFormatter::render_with_baseline_with_opts(
            &[(issue, false)],
            &[CYCLOMATIC_META],
            JsonRenderOpts::default(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(
            parsed["findings"][0]["docs_url"],
            "https://tajd.github.io/cofferdam/checks/Refactor.CyclomaticComplexity"
        );
    }

    #[test]
    fn json_baseline_plugin_finding_has_no_docs_url() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.TenantIsolation");
        let output = JsonFormatter::render_with_baseline_with_opts(
            &[(issue, false)],
            &[],
            JsonRenderOpts::default(),
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert!(
            parsed["findings"][0]["docs_url"].is_null(),
            "plugin checks must not receive a docs_url in baseline mode"
        );
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
            &[],
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
    fn json_bytes_issue_renders_start_and_end_byte() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render(&[issue]);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["findings"][0]["start_byte"], 0);
        assert_eq!(parsed["findings"][0]["end_byte"], 10);
        assert!(parsed["findings"][0]["end_line"].is_null());
        assert!(parsed["findings"][0]["end_col"].is_null());
    }

    #[test]
    fn json_linecol_issue_omits_byte_fields_and_carries_end_line_col() {
        let loc = Location {
            uri: cofferdam_core::Uri::new("gen://out.ts"),
            range: cofferdam_core::LocationRange::LineCol {
                start_line: 4,
                start_col: 2,
                end_line: 6,
                end_col: 9,
            },
        };
        let issue = Issue {
            location: loc,
            file: PathBuf::from("out.ts"),
            message: "generated finding".into(),
            check_id: "Warning.Test".into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        };
        let output = JsonFormatter::render(&[issue]);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        let finding = &parsed["findings"][0];
        assert_eq!(finding["line"], 4);
        assert_eq!(finding["column"], 2);
        assert_eq!(finding["end_line"], 6);
        assert_eq!(finding["end_col"], 9);
        assert!(
            finding["start_byte"].is_null(),
            "LineCol must not fabricate a byte offset, got:\n{output}"
        );
        assert!(finding["end_byte"].is_null());
    }

    #[test]
    fn json_custom_issue_degrades_without_panicking_or_fabricating_data() {
        let loc = Location {
            uri: cofferdam_core::Uri::new("sql://migrations"),
            range: cofferdam_core::LocationRange::Custom {
                ns: "sql".into(),
                id: "stmt:3".into(),
            },
        };
        let issue = Issue {
            location: loc,
            file: PathBuf::from("migrations.sql"),
            message: "custom finding".into(),
            check_id: "Warning.Test".into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        };
        let output = JsonFormatter::render(&[issue]);
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        let finding = &parsed["findings"][0];
        assert_eq!(finding["line"], 0);
        assert_eq!(finding["column"], 0);
        assert!(finding["start_byte"].is_null());
        assert!(finding["end_byte"].is_null());
        assert!(finding["end_line"].is_null());
        assert!(finding["end_col"].is_null());
    }

    #[test]
    fn json_formatter_baseline_includes_truncated_from() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = JsonFormatter::render_with_baseline_with_opts(
            &[(issue, false)],
            &[],
            JsonRenderOpts {
                pretty: false,
                truncated_from: Some(99),
            },
        );
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
        assert_eq!(parsed["summary"]["truncated_from"], 99);
    }
}
