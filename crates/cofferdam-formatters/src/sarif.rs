//! SARIF 2.1.0 formatter.
//!
//! Emits an OASIS-standard SARIF document so cofferdam findings can be
//! ingested by GitHub Code Scanning, Azure DevOps, GitLab, SonarQube,
//! and the VS Code Sarif Viewer with no bespoke parsing.
//!
//! Schema reference:
//! <https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html>
//!
//! ## Severity → SARIF level mapping
//!
//! SARIF has a coarser scale than cofferdam (`note | warning | error |
//! none`). Cofferdam's five-level severity collapses as follows:
//!
//! | cofferdam | SARIF level |
//! |-----------|-------------|
//! | `info`    | `note`      |
//! | `low`     | `note`      |
//! | `medium`  | `warning`   |
//! | `high`    | `error`     |
//! | `critical`| `error`     |
//!
//! ## Rules table
//!
//! `runs[].tool.driver.rules[]` is populated from the supplied
//! `&[CheckMeta]` slice (typically `cofferdam_checks::all_builtins()`'s
//! metadata). When metadata is absent for a check id seen in the
//! findings, a minimal rule entry with just the id is synthesised so the
//! result still points at *something* — better a stub rule than a
//! dangling `ruleId`.
//!
//! ## Fingerprints
//!
//! Each result emits a `partialFingerprints` entry under the key
//! `cofferdam/v1` derived from `(check_id, file, line, message)`. GitHub
//! Code Scanning uses these to track the same finding across runs even
//! when surrounding line numbers shift.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use cofferdam_core::{docs_url, CheckMeta, Issue, RelatedSpan, Severity};
use serde::Serialize;

use crate::common::{category_str, normalize_path};

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_VERSION: &str = "2.1.0";
const TOOL_NAME: &str = "cofferdam";
const TOOL_INFO_URI: &str = "https://github.com/TAJD/cofferdam";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Stable namespace for `partialFingerprints`. Bump only on a
/// fingerprinting algorithm change so existing GitHub Code Scanning
/// alerts don't all re-trigger.
const FINGERPRINT_KEY: &str = "cofferdam/v1";

#[derive(Serialize)]
pub(crate) struct SarifReport<'a> {
    #[serde(rename = "$schema")]
    pub schema: &'static str,
    pub version: &'static str,
    pub runs: Vec<SarifRun<'a>>,
}

#[derive(Serialize)]
pub(crate) struct SarifRun<'a> {
    pub tool: SarifTool<'a>,
    pub results: Vec<SarifResult<'a>>,
}

#[derive(Serialize)]
pub(crate) struct SarifTool<'a> {
    pub driver: SarifDriver<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifDriver<'a> {
    pub name: &'static str,
    pub version: &'static str,
    pub information_uri: &'static str,
    pub rules: Vec<SarifRule<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifRule<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub short_description: SarifText<'a>,
    pub full_description: SarifText<'a>,
    pub default_configuration: SarifLevel,
    pub properties: SarifRuleProperties,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_uri: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SarifText<'a> {
    pub text: &'a str,
}

#[derive(Serialize)]
pub(crate) struct SarifLevel {
    pub level: &'static str,
}

#[derive(Serialize)]
pub(crate) struct SarifRuleProperties {
    pub category: &'static str,
    pub priority: i8,
    pub tags: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifResult<'a> {
    pub rule_id: &'a str,
    pub level: &'static str,
    pub message: SarifMessage<'a>,
    pub locations: Vec<SarifLocation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_locations: Vec<SarifLocation>,
    pub partial_fingerprints: BTreeMap<&'static str, String>,
}

#[derive(Serialize)]
pub(crate) struct SarifMessage<'a> {
    pub text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifLocation {
    pub physical_location: SarifPhysicalLocation,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifPhysicalLocation {
    pub artifact_location: SarifArtifactLocation,
    pub region: SarifRegion,
}

#[derive(Serialize)]
pub(crate) struct SarifArtifactLocation {
    pub uri: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SarifRegion {
    pub start_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// Byte offset/length, present only for `Bytes` locations. `LineCol`
    /// and `Custom` locations have no byte representation — omit rather
    /// than emit a fake `0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_length: Option<u32>,
}

/// Options for the SARIF formatter. `pretty` controls whether the JSON
/// document is pretty-printed; the default is compact (single-line)
/// output to match the rest of the formatter family.
#[derive(Copy, Clone, Debug, Default)]
pub struct SarifRenderOpts {
    pub pretty: bool,
}

/// SARIF 2.1.0 formatter. Construct via the unit type and call
/// `render` / `render_with_opts` to serialize findings to a SARIF
/// JSON document. The internal wire-format structs (`SarifReport`,
/// `SarifRun`, etc.) are `pub(crate)` — only this formatter and
/// `SarifRenderOpts` are part of the crate's public surface.
pub struct SarifFormatter;

impl SarifFormatter {
    /// Render findings as a SARIF 2.1.0 document. `metas` is the slice
    /// of check metadata used to populate `runs[].tool.driver.rules[]`
    /// — typically `cofferdam_checks::all_builtins()`'s metadata. Pass
    /// `&[]` to synthesise minimal rule entries from the issues alone.
    pub fn render(issues: &[Issue], metas: &[CheckMeta]) -> String {
        Self::render_with_opts(issues, metas, SarifRenderOpts::default())
    }

    /// Pretty-printed variant, suitable for human inspection.
    pub fn render_pretty(issues: &[Issue], metas: &[CheckMeta]) -> String {
        Self::render_with_opts(issues, metas, SarifRenderOpts { pretty: true })
    }

    /// Render with explicit options (compact vs pretty). `metas`
    /// populates `runs[].tool.driver.rules[]` so consumers see one
    /// rule entry per registered check id; missing meta falls back
    /// to a minimal stub keyed by id alone.
    pub fn render_with_opts(
        issues: &[Issue],
        metas: &[CheckMeta],
        opts: SarifRenderOpts,
    ) -> String {
        let report = build_report(issues, metas);
        if opts.pretty {
            serde_json::to_string_pretty(&report).expect("SarifReport serializes infallibly")
        } else {
            serde_json::to_string(&report).expect("SarifReport serializes infallibly")
        }
    }
}

fn build_report<'a>(issues: &'a [Issue], metas: &'a [CheckMeta]) -> SarifReport<'a> {
    let meta_index: BTreeMap<&str, &CheckMeta> = metas.iter().map(|m| (m.id, m)).collect();

    let mut rule_ids: BTreeSet<&str> = BTreeSet::new();
    for issue in issues {
        rule_ids.insert(issue.check_id.as_str());
    }

    let mut rules: Vec<SarifRule<'_>> = Vec::with_capacity(rule_ids.len());
    for id in &rule_ids {
        let rule = match meta_index.get(id) {
            Some(meta) => SarifRule {
                id: meta.id,
                name: short_name(meta.id),
                short_description: SarifText {
                    text: meta.explanation,
                },
                full_description: SarifText {
                    text: meta.explanation,
                },
                default_configuration: SarifLevel {
                    level: severity_to_sarif(meta.default_severity),
                },
                properties: SarifRuleProperties {
                    category: category_str(Some(meta.category)),
                    priority: meta.base_priority,
                    tags: vec![category_str(Some(meta.category))],
                },
                help_uri: Some(docs_url(meta.id)),
            },
            None => SarifRule {
                id,
                name: short_name(id),
                short_description: SarifText {
                    text: "(no metadata available)",
                },
                full_description: SarifText {
                    text: "(no metadata available)",
                },
                default_configuration: SarifLevel { level: "warning" },
                properties: SarifRuleProperties {
                    category: "unknown",
                    priority: 0,
                    tags: vec!["unknown"],
                },
                help_uri: None,
            },
        };
        rules.push(rule);
    }

    let results: Vec<SarifResult<'_>> = issues
        .iter()
        .map(|i| SarifResult {
            rule_id: i.check_id.as_str(),
            level: severity_to_sarif(i.severity),
            message: SarifMessage {
                text: i.message.as_str(),
            },
            locations: vec![SarifLocation {
                physical_location: SarifPhysicalLocation {
                    artifact_location: SarifArtifactLocation {
                        uri: normalize_path(&i.file),
                    },
                    region: region_for(&i.location),
                },
            }],
            related_locations: i.related.iter().map(map_related_location).collect(),
            partial_fingerprints: fingerprints_for(i),
        })
        .collect();

    SarifReport {
        schema: SARIF_SCHEMA,
        version: SARIF_VERSION,
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: TOOL_NAME,
                    version: TOOL_VERSION,
                    information_uri: TOOL_INFO_URI,
                    rules,
                },
            },
            results,
        }],
    }
}

fn map_related_location(r: &RelatedSpan) -> SarifLocation {
    SarifLocation {
        physical_location: SarifPhysicalLocation {
            artifact_location: SarifArtifactLocation {
                uri: normalize_path(&r.file),
            },
            region: region_for(&r.location),
        },
    }
}

/// Build a SARIF `region` from a `Location`, matching on its
/// `LocationRange` variant so each renders coherently: `Bytes` keeps the
/// historical `byteOffset`/`byteLength` shape; `LineCol` emits
/// `startLine`/`startColumn`/`endLine`/`endColumn` and omits the byte
/// fields (there is no byte data to report); `Custom` has neither byte
/// nor line data and degrades to `startLine: 0` with everything else
/// omitted.
fn region_for(location: &cofferdam_core::Location) -> SarifRegion {
    match location.byte_range() {
        Some((start, end)) => SarifRegion {
            start_line: location.line(),
            start_column: nonzero(location.column()),
            end_line: None,
            end_column: None,
            byte_offset: Some(start),
            byte_length: Some(end.saturating_sub(start)),
        },
        None => match location.end_line_col() {
            Some((end_line, end_col)) => SarifRegion {
                start_line: location.line(),
                start_column: nonzero(location.column()),
                end_line: Some(end_line),
                end_column: nonzero(end_col),
                byte_offset: None,
                byte_length: None,
            },
            None => SarifRegion {
                start_line: location.line(),
                start_column: nonzero(location.column()),
                end_line: None,
                end_column: None,
                byte_offset: None,
                byte_length: None,
            },
        },
    }
}

/// SARIF requires `startColumn >= 1`. Cofferdam columns are already
/// 1-based but defensively drop a 0 instead of emitting an invalid
/// SARIF document.
fn nonzero(col: u32) -> Option<u32> {
    if col == 0 {
        None
    } else {
        Some(col)
    }
}

fn severity_to_sarif(sev: Severity) -> &'static str {
    match sev {
        Severity::Info | Severity::Low => "note",
        Severity::Medium => "warning",
        Severity::High | Severity::Critical => "error",
    }
}

fn short_name(check_id: &str) -> &str {
    check_id.rsplit('.').next().unwrap_or(check_id)
}

fn fingerprints_for(issue: &Issue) -> BTreeMap<&'static str, String> {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    issue.check_id.hash(&mut h);
    normalize_path(&issue.file).hash(&mut h);
    issue.location.line().hash(&mut h);
    issue.message.hash(&mut h);
    let mut map = BTreeMap::new();
    map.insert(FINGERPRINT_KEY, format!("{:016x}", h.finish()));
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Category, Location, Priority, Severity, Span};
    use std::path::PathBuf;

    fn meta(id: &'static str, cat: Category) -> CheckMeta {
        CheckMeta {
            id,
            category: cat,
            base_priority: 15,
            default_severity: Severity::Medium,
            explanation: "test rule",
            body: "",
            requires_types: false,
            consistency: false,
            options: &[],
            autofix: false,
            pure_run: false,
        }
    }

    fn issue(file: &str, check_id: &str, sev: Severity) -> Issue {
        let path = PathBuf::from(file);
        Issue {
            location: Location::from_span(
                &path,
                Span {
                    line: 42,
                    column: 7,
                    start_byte: 800,
                    end_byte: 806,
                },
            ),
            file: path,
            message: "use `===` instead of `==`".into(),
            check_id: check_id.into(),
            severity: sev,
            priority: Priority(15),
            related: Vec::new(),
        }
    }

    #[test]
    fn sarif_root_envelope_is_well_formed() {
        let issues = [issue("src/auth.ts", "Warning.TripleEquals", Severity::High)];
        let metas = [meta("Warning.TripleEquals", Category::Warning)];
        let out = SarifFormatter::render(&issues, &metas);
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["$schema"], SARIF_SCHEMA);
        assert_eq!(v["version"], "2.1.0");
        assert_eq!(v["runs"].as_array().unwrap().len(), 1);
        assert_eq!(v["runs"][0]["tool"]["driver"]["name"], "cofferdam");
        assert_eq!(
            v["runs"][0]["tool"]["driver"]["informationUri"],
            TOOL_INFO_URI
        );
    }

    #[test]
    fn sarif_severity_collapse_matches_table() {
        assert_eq!(severity_to_sarif(Severity::Info), "note");
        assert_eq!(severity_to_sarif(Severity::Low), "note");
        assert_eq!(severity_to_sarif(Severity::Medium), "warning");
        assert_eq!(severity_to_sarif(Severity::High), "error");
        assert_eq!(severity_to_sarif(Severity::Critical), "error");
    }

    #[test]
    fn sarif_emits_one_rule_per_unique_check_id() {
        let issues = [
            issue("a.ts", "Warning.TripleEquals", Severity::High),
            issue("b.ts", "Warning.TripleEquals", Severity::High),
            issue("c.ts", "Refactor.CyclomaticComplexity", Severity::Medium),
        ];
        let metas = [
            meta("Warning.TripleEquals", Category::Warning),
            meta("Refactor.CyclomaticComplexity", Category::Refactor),
        ];
        let out = SarifFormatter::render(&issues, &metas);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn sarif_synthesises_rule_when_meta_missing() {
        let issues = [issue("a.ts", "Custom.Plugin", Severity::Medium)];
        let out = SarifFormatter::render(&issues, &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["id"], "Custom.Plugin");
        // Plugin checks have no catalog page — helpUri must be absent.
        assert!(
            rules[0]["helpUri"].is_null(),
            "plugin rules must not emit a helpUri that 404s"
        );
    }

    #[test]
    fn sarif_rule_help_uri_points_at_docs_catalog() {
        let issues = [issue("src/auth.ts", "Warning.TripleEquals", Severity::High)];
        let metas = [meta("Warning.TripleEquals", Category::Warning)];
        let out = SarifFormatter::render(&issues, &metas);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        assert_eq!(
            rules[0]["helpUri"],
            "https://tajd.github.io/cofferdam/checks/Warning.TripleEquals"
        );
    }

    #[test]
    fn sarif_normalizes_windows_paths_in_uri() {
        let mut i = issue(r"C:\Users\demo\src\foo.ts", "Warning.X", Severity::Medium);
        i.file = PathBuf::from(r"C:\Users\demo\src\foo.ts");
        let out = SarifFormatter::render(&[i], &[]);
        assert!(out.contains("C:/Users/demo/src/foo.ts"));
        assert!(!out.contains(r"\Users"));
    }

    #[test]
    fn sarif_result_carries_partial_fingerprint() {
        let issues = [issue("src/a.ts", "Warning.X", Severity::Medium)];
        let out = SarifFormatter::render(&issues, &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let fp = &v["runs"][0]["results"][0]["partialFingerprints"][FINGERPRINT_KEY];
        assert!(
            fp.is_string(),
            "fingerprint should be a hex string, got {fp:?}"
        );
        assert_eq!(fp.as_str().unwrap().len(), 16);
    }

    #[test]
    fn sarif_empty_findings_emits_empty_results() {
        let out = SarifFormatter::render(&[], &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["runs"][0]["results"].as_array().unwrap().is_empty());
        assert!(v["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sarif_region_uses_byte_offset_and_length() {
        let issues = [issue("src/a.ts", "Warning.X", Severity::Medium)];
        let out = SarifFormatter::render(&issues, &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let region = &v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 42);
        assert_eq!(region["startColumn"], 7);
        assert_eq!(region["byteOffset"], 800);
        assert_eq!(region["byteLength"], 6);
        assert!(region["endLine"].is_null());
        assert!(region["endColumn"].is_null());
    }

    #[test]
    fn sarif_linecol_region_uses_start_end_line_col_and_omits_byte_fields() {
        let loc = Location {
            uri: cofferdam_core::Uri::new("gen://out.ts"),
            range: cofferdam_core::LocationRange::LineCol {
                start_line: 4,
                start_col: 2,
                end_line: 6,
                end_col: 9,
            },
        };
        let path = PathBuf::from("out.ts");
        let i = Issue {
            location: loc,
            file: path,
            message: "generated finding".into(),
            check_id: "Warning.X".into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        };
        let out = SarifFormatter::render(&[i], &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let region = &v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 4);
        assert_eq!(region["startColumn"], 2);
        assert_eq!(region["endLine"], 6);
        assert_eq!(region["endColumn"], 9);
        assert!(
            region["byteOffset"].is_null(),
            "LineCol region must not fabricate a byte offset, got:\n{out}"
        );
        assert!(region["byteLength"].is_null());
    }

    #[test]
    fn sarif_custom_region_degrades_without_panicking_or_fabricating_data() {
        let loc = Location {
            uri: cofferdam_core::Uri::new("sql://migrations"),
            range: cofferdam_core::LocationRange::Custom {
                ns: "sql".into(),
                id: "stmt:3".into(),
            },
        };
        let path = PathBuf::from("migrations.sql");
        let i = Issue {
            location: loc,
            file: path,
            message: "custom finding".into(),
            check_id: "Warning.X".into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        };
        let out = SarifFormatter::render(&[i], &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let region = &v["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], 0);
        assert!(region["startColumn"].is_null());
        assert!(region["endLine"].is_null());
        assert!(region["endColumn"].is_null());
        assert!(region["byteOffset"].is_null());
        assert!(region["byteLength"].is_null());
    }
}
