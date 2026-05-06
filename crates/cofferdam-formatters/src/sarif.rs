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
use std::path::Path;

use cofferdam_core::{Category, CheckMeta, Issue, RelatedSpan, Severity};
use serde::Serialize;

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
pub struct SarifReport<'a> {
    #[serde(rename = "$schema")]
    pub schema: &'static str,
    pub version: &'static str,
    pub runs: Vec<SarifRun<'a>>,
}

#[derive(Serialize)]
pub struct SarifRun<'a> {
    pub tool: SarifTool<'a>,
    pub results: Vec<SarifResult<'a>>,
}

#[derive(Serialize)]
pub struct SarifTool<'a> {
    pub driver: SarifDriver<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifDriver<'a> {
    pub name: &'static str,
    pub version: &'static str,
    pub information_uri: &'static str,
    pub rules: Vec<SarifRule<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRule<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub short_description: SarifText<'a>,
    pub full_description: SarifText<'a>,
    pub default_configuration: SarifLevel,
    pub properties: SarifRuleProperties,
}

#[derive(Serialize)]
pub struct SarifText<'a> {
    pub text: &'a str,
}

#[derive(Serialize)]
pub struct SarifLevel {
    pub level: &'static str,
}

#[derive(Serialize)]
pub struct SarifRuleProperties {
    pub category: &'static str,
    pub priority: i8,
    pub tags: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifResult<'a> {
    pub rule_id: &'a str,
    pub level: &'static str,
    pub message: SarifMessage<'a>,
    pub locations: Vec<SarifLocation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_locations: Vec<SarifLocation>,
    pub partial_fingerprints: BTreeMap<&'static str, String>,
}

#[derive(Serialize)]
pub struct SarifMessage<'a> {
    pub text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifLocation {
    pub physical_location: SarifPhysicalLocation,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifPhysicalLocation {
    pub artifact_location: SarifArtifactLocation,
    pub region: SarifRegion,
}

#[derive(Serialize)]
pub struct SarifArtifactLocation {
    pub uri: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SarifRegion {
    pub start_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    pub byte_offset: u32,
    pub byte_length: u32,
}

/// Options for the SARIF formatter. `pretty` controls whether the JSON
/// document is pretty-printed; the default is compact (single-line)
/// output to match the rest of the formatter family.
#[derive(Copy, Clone, Debug, Default)]
pub struct SarifRenderOpts {
    pub pretty: bool,
}

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
                    category: category_str(meta.category),
                    priority: meta.base_priority,
                    tags: vec![category_str(meta.category)],
                },
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
                    region: SarifRegion {
                        start_line: i.span.line,
                        start_column: nonzero(i.span.column),
                        byte_offset: i.span.start_byte,
                        byte_length: i.span.end_byte.saturating_sub(i.span.start_byte),
                    },
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
            region: SarifRegion {
                start_line: r.span.line,
                start_column: nonzero(r.span.column),
                byte_offset: r.span.start_byte,
                byte_length: r.span.end_byte.saturating_sub(r.span.start_byte),
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

fn category_str(cat: Category) -> &'static str {
    match cat {
        Category::Consistency => "consistency",
        Category::Design => "design",
        Category::Readability => "readability",
        Category::Refactor => "refactor",
        Category::Warning => "warning",
    }
}

fn short_name(check_id: &str) -> &str {
    check_id.rsplit('.').next().unwrap_or(check_id)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn fingerprints_for(issue: &Issue) -> BTreeMap<&'static str, String> {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    issue.check_id.hash(&mut h);
    normalize_path(&issue.file).hash(&mut h);
    issue.span.line.hash(&mut h);
    issue.message.hash(&mut h);
    let mut map = BTreeMap::new();
    map.insert(FINGERPRINT_KEY, format!("{:016x}", h.finish()));
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Category, Priority, Severity, Span};
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
            observes_findings: false,
        }
    }

    fn issue(file: &str, check_id: &str, sev: Severity) -> Issue {
        Issue {
            file: PathBuf::from(file),
            span: Span {
                line: 42,
                column: 7,
                start_byte: 800,
                end_byte: 806,
            },
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
    }
}
