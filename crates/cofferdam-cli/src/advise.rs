//! `cofferdam advise <path>` — JIT architectural advisory for agents.
//!
//! Emits, for each input file, the rules that APPLY to the file
//! independent of whether any current code violates them. Designed for
//! agentic edit loops: an LLM agent shells out before editing a file, gets
//! back the layer membership and per-rule constraints, and can adjust its
//! plan before writing code.
//!
//! Static projection — does NOT run the engine, parse files, or build the
//! project graph. The output is `(CheckMeta + resolved options + layer
//! state)` over each requested path. That keeps single-file advise well
//! under the 200ms budget; graph-aware advisory is a follow-up bead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::layers::{self, LayerMatcher};
use cofferdam_core::{Category, CheckMeta, CheckOptions, OptionDefault, OptionValue, Severity};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::{discover, DiscoveryOptions, ProjectConfig};
use serde::Serialize;

pub struct AdviseArgs {
    pub paths: Vec<PathBuf>,
    pub format: AdviseFormat,
    pub pretty: bool,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
    pub hidden: bool,
    pub no_ignore: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AdviseFormat {
    Text,
    Json,
}

pub fn run(args: AdviseArgs) -> ExitCode {
    let AdviseArgs {
        paths,
        format,
        pretty,
        config_path,
        no_config,
        hidden,
        no_ignore,
    } = args;

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };

    // Split each input into either a literal path (passed straight to
    // discover) or a glob pattern that we walk-and-filter against. Shell
    // expansion (`src/**/*.ts` in PowerShell or bash) means most callers
    // never hit the glob arm; the arm exists for `cmd.exe`, programmatic
    // invocation, and quoted patterns.
    let (literal_roots, glob_specs) = split_globs(&roots);

    let opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };

    let mut files: Vec<PathBuf> = if literal_roots.is_empty() {
        Vec::new()
    } else {
        match discover(&literal_roots, &opts) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    };

    if !glob_specs.is_empty() {
        let walk_roots = vec![PathBuf::from(".")];
        let walked = match discover(&walk_roots, &opts) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        };
        let glob_set = match build_glob_set(&glob_specs) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: invalid glob pattern: {e}");
                return ExitCode::from(2);
            }
        };
        for p in walked {
            let key = p.to_string_lossy().replace('\\', "/");
            if glob_set.is_match(&key) {
                files.push(p);
            }
        }
    }

    files.sort();
    files.dedup();

    let (project_config, _config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let checks = all_builtins();

    // Resolve options per check up front. `options_for` returns defaults
    // when the project config is silent.
    let resolved: Vec<(CheckMeta, CheckOptions, Severity)> = checks
        .iter()
        .map(|c| {
            let meta = *c.meta();
            let options = match project_config.as_ref() {
                Some(p) => cfg::options_for(p, Path::new(""), meta.id, meta.options)
                    .unwrap_or_else(|_| CheckOptions::defaults_from(meta.options)),
                None => CheckOptions::defaults_from(meta.options),
            };
            let severity = project_config
                .as_ref()
                .and_then(|p| p.severity_overrides.get(meta.id).copied())
                .unwrap_or(meta.default_severity);
            (meta, options, severity)
        })
        .collect();

    let layers_cfg = project_config.as_ref().and_then(|p| p.layers.as_ref());
    let layer_matchers: Vec<LayerMatcher> =
        layers_cfg.map(layers::build_matchers).unwrap_or_default();

    let advisories: Vec<FileAdvisory> = files
        .iter()
        .map(|file| build_advisory(file, &resolved, layers_cfg, &layer_matchers))
        .collect();

    match format {
        AdviseFormat::Json => emit_json(&advisories, pretty),
        AdviseFormat::Text => emit_text(&advisories),
    }

    ExitCode::SUCCESS
}

fn build_advisory(
    file: &Path,
    resolved: &[(CheckMeta, CheckOptions, Severity)],
    layers_cfg: Option<&cofferdam_core::LayersConfig>,
    layer_matchers: &[LayerMatcher],
) -> FileAdvisory {
    let layer =
        layers_cfg.and_then(|cfg| layers::layer_for(layer_matchers, &cfg.project_root, file));

    // public_api is forward-looking — cofferdam.invariants.toml (cd-9ph)
    // will populate the allowlist; until then no file is on it.
    let public_api = false;

    let constraints: Vec<Constraint> = resolved
        .iter()
        .map(|(meta, options, severity)| {
            build_constraint(file, meta, options, *severity, layer.as_deref(), layers_cfg)
        })
        .collect();

    FileAdvisory {
        path: file.to_string_lossy().replace('\\', "/"),
        layer,
        public_api,
        constraints,
    }
}

fn build_constraint(
    file: &Path,
    meta: &CheckMeta,
    options: &CheckOptions,
    severity: Severity,
    layer: Option<&str>,
    layers_cfg: Option<&cofferdam_core::LayersConfig>,
) -> Constraint {
    let parameters = build_parameters(meta, options);

    let mut allowed: Option<Vec<String>> = None;
    let mut forbidden: Option<Vec<String>> = None;
    let mut exempt: Option<bool> = None;
    let mut exempt_reason: Option<&'static str> = None;

    let applies = match meta.id {
        "Design.LayerViolation" => match (layer, layers_cfg) {
            (Some(layer_name), Some(cfg)) => {
                let allowed_list: Vec<String> =
                    cfg.allow.get(layer_name).cloned().unwrap_or_default();
                let all_layers: Vec<String> = cfg.layers.keys().cloned().collect();
                let forbidden_list: Vec<String> = all_layers
                    .iter()
                    .filter(|l| l.as_str() != layer_name && !allowed_list.iter().any(|a| a == *l))
                    .cloned()
                    .collect();
                let s = if allowed_list.is_empty() {
                    format!(
                        "imports must stay within layer `{}` (no cross-layer imports allowed)",
                        layer_name
                    )
                } else {
                    format!("imports must target layer(s) [{}]", allowed_list.join(", "))
                };
                allowed = Some(allowed_list);
                forbidden = Some(forbidden_list);
                s
            }
            _ => "no layer rules apply (file is not in any declared layer)".to_string(),
        },
        "Design.OrphanExport" => {
            let test_patterns = options
                .get_string_list("test_file_patterns")
                .map(|xs| xs.to_vec())
                .unwrap_or_default();
            let framework_patterns = options
                .get_string_list("framework_entry_patterns")
                .map(|xs| xs.to_vec())
                .unwrap_or_default();
            if matches_substring(file, &framework_patterns) {
                exempt = Some(true);
                exempt_reason = Some("framework entry point");
            } else if matches_substring(file, &test_patterns) {
                exempt = Some(true);
                exempt_reason = Some("test file");
            } else {
                exempt = Some(false);
            }
            "every export must be imported somewhere in-project".to_string()
        }
        _ => default_applies(meta, options),
    };

    Constraint {
        rule: meta.id,
        category: category_str(meta.category),
        severity: severity.as_str(),
        applies,
        rationale: meta.explanation,
        parameters,
        allowed,
        forbidden,
        exempt,
        exempt_reason,
    }
}

/// Render a generic `applies` line from option values. For checks with a
/// single int `limit` option we produce "limit N"; for everything else we
/// fall back to the meta `explanation` (which the agent also sees as
/// `rationale`, but `applies` is the load-bearing one-liner).
fn default_applies(meta: &CheckMeta, options: &CheckOptions) -> String {
    if let Some(limit) = options.get_int("limit") {
        return format!("limit {}", limit);
    }
    if meta.options.is_empty() {
        return meta.explanation.to_string();
    }
    let parts: Vec<String> = meta
        .options
        .iter()
        .filter_map(|spec| match options.get(spec.name)? {
            OptionValue::Bool(b) => Some(format!("{}={}", spec.name, b)),
            OptionValue::Int(i) => Some(format!("{}={}", spec.name, i)),
            OptionValue::String(s) => Some(format!("{}=\"{}\"", spec.name, s)),
            OptionValue::StringList(_) | OptionValue::IntList(_) => None,
        })
        .collect();
    if parts.is_empty() {
        meta.explanation.to_string()
    } else {
        parts.join(", ")
    }
}

fn build_parameters(
    meta: &CheckMeta,
    options: &CheckOptions,
) -> Option<BTreeMap<String, ParamValue>> {
    if meta.options.is_empty() {
        return None;
    }
    let mut out = BTreeMap::new();
    for spec in meta.options {
        if let Some(value) = options.get(spec.name) {
            out.insert(spec.name.to_string(), to_param_value(value));
        } else {
            out.insert(spec.name.to_string(), default_to_param(&spec.default));
        }
    }
    Some(out)
}

fn to_param_value(v: &OptionValue) -> ParamValue {
    match v {
        OptionValue::Bool(b) => ParamValue::Bool(*b),
        OptionValue::Int(i) => ParamValue::Int(*i),
        OptionValue::String(s) => ParamValue::String(s.clone()),
        OptionValue::StringList(xs) => ParamValue::StringList(xs.clone()),
        OptionValue::IntList(xs) => ParamValue::IntList(xs.clone()),
    }
}

fn default_to_param(d: &OptionDefault) -> ParamValue {
    match *d {
        OptionDefault::Bool(b) => ParamValue::Bool(b),
        OptionDefault::Int(i) => ParamValue::Int(i),
        OptionDefault::String(s) => ParamValue::String(s.to_string()),
        OptionDefault::StringList(xs) => {
            ParamValue::StringList(xs.iter().map(|s| s.to_string()).collect())
        }
        OptionDefault::IntList(xs) => ParamValue::IntList(xs.to_vec()),
    }
}

fn matches_substring(path: &Path, patterns: &[String]) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|p| s.contains(p))
}

fn category_str(c: Category) -> &'static str {
    match c {
        Category::Consistency => "consistency",
        Category::Design => "design",
        Category::Readability => "readability",
        Category::Refactor => "refactor",
        Category::Warning => "warning",
    }
}

fn split_globs(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut literal = Vec::new();
    let mut globs = Vec::new();
    for r in roots {
        let s = r.to_string_lossy().to_string();
        if s.contains('*') || s.contains('?') || s.contains('[') {
            globs.push(s.replace('\\', "/"));
        } else {
            literal.push(r.clone());
        }
    }
    (literal, globs)
}

fn build_glob_set(specs: &[String]) -> Result<globset::GlobSet, globset::Error> {
    let mut builder = globset::GlobSetBuilder::new();
    for s in specs {
        builder.add(globset::Glob::new(s)?);
        // Also accept `./` prefixed forms — discover yields paths without
        // a leading `./` on Unix-ish output, but the user might have typed
        // `./src/**/*.ts`. Add both forms so either matches.
        if let Some(stripped) = s.strip_prefix("./") {
            builder.add(globset::Glob::new(stripped)?);
        } else {
            builder.add(globset::Glob::new(&format!("./{}", s))?);
        }
    }
    builder.build()
}

#[derive(Serialize)]
struct FileAdvisory {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    layer: Option<String>,
    public_api: bool,
    constraints: Vec<Constraint>,
}

#[derive(Serialize)]
struct Constraint {
    rule: &'static str,
    category: &'static str,
    severity: &'static str,
    applies: String,
    rationale: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<BTreeMap<String, ParamValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forbidden: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exempt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exempt_reason: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ParamValue {
    Bool(bool),
    Int(i64),
    String(String),
    StringList(Vec<String>),
    IntList(Vec<i64>),
}

fn emit_json(advisories: &[FileAdvisory], pretty: bool) {
    let s = if pretty {
        serde_json::to_string_pretty(advisories)
    } else {
        serde_json::to_string(advisories)
    }
    .expect("FileAdvisory serializes infallibly");
    println!("{}", s);
}

fn emit_text(advisories: &[FileAdvisory]) {
    use std::fmt::Write as _;
    let mut out = String::new();
    for adv in advisories {
        let _ = writeln!(out, "{}", adv.path);
        match &adv.layer {
            Some(l) => {
                let _ = writeln!(out, "  Layer:       {}", l);
            }
            None => {
                let _ = writeln!(out, "  Layer:       (none)");
            }
        }
        let _ = writeln!(
            out,
            "  Public API:  {}",
            if adv.public_api { "yes" } else { "no" }
        );
        if adv.constraints.is_empty() {
            let _ = writeln!(out, "  Constraints: (none)");
        } else {
            let _ = writeln!(out, "  Constraints:");
            for c in &adv.constraints {
                let _ = writeln!(
                    out,
                    "    {} ({}, severity {}) — {}",
                    c.rule, c.category, c.severity, c.applies
                );
                if let Some(true) = c.exempt {
                    let _ = writeln!(out, "      exempt: {}", c.exempt_reason.unwrap_or("yes"));
                }
                if let Some(forbidden) = &c.forbidden {
                    if !forbidden.is_empty() {
                        let _ = writeln!(out, "      forbidden layers: {}", forbidden.join(", "));
                    }
                }
            }
        }
        let _ = writeln!(out);
    }
    print!("{}", out);
}

/// Mirror of `main.rs::resolve_and_load_config` — kept private here so the
/// advise subcommand can be exercised independently in tests without
/// pulling in the rest of the CLI's check-mode machinery.
fn resolve_and_load_config(
    explicit: Option<&Path>,
    no_config: bool,
) -> Result<(Option<ProjectConfig>, Option<PathBuf>), ()> {
    if no_config {
        return Ok((None, None));
    }
    let path = match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => std::env::current_dir().ok().and_then(|d| cfg::discover(&d)),
    };
    let path = match path {
        Some(p) => p,
        None => return Ok((None, None)),
    };
    match cfg::load(&path) {
        Ok(c) => Ok((Some(c), Some(path))),
        Err(e) => {
            if explicit.is_some() {
                eprintln!("error: {e}");
                Err(())
            } else {
                eprintln!("warning: ignoring config ({e})");
                Ok((None, None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::LayersConfig;
    use cofferdam_core::{OptionDefault, OptionKind, OptionSpec};
    use std::collections::BTreeMap;

    const LIMIT_OPTS: &[OptionSpec] = &[OptionSpec {
        name: "limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(10),
        doc: "test",
    }];

    const LIMIT_META: CheckMeta = CheckMeta {
        id: "Refactor.Test",
        category: Category::Refactor,
        base_priority: 0,
        default_severity: Severity::Medium,
        explanation: "test rationale",
        body: "",
        requires_types: false,
        consistency: false,
        options: LIMIT_OPTS,
    };

    fn fixture_layers() -> LayersConfig {
        let mut layers = BTreeMap::new();
        layers.insert("app".to_string(), vec!["src/app/**".to_string()]);
        layers.insert("domain".to_string(), vec!["src/domain/**".to_string()]);
        layers.insert("infra".to_string(), vec!["src/infra/**".to_string()]);
        let mut allow = BTreeMap::new();
        allow.insert(
            "app".to_string(),
            vec!["domain".to_string(), "infra".to_string()],
        );
        allow.insert("domain".to_string(), vec!["infra".to_string()]);
        LayersConfig {
            project_root: PathBuf::from("/repo"),
            layers,
            allow,
        }
    }

    #[test]
    fn default_applies_uses_limit_when_present() {
        let opts = CheckOptions::defaults_from(LIMIT_OPTS);
        assert_eq!(default_applies(&LIMIT_META, &opts), "limit 10");
    }

    #[test]
    fn parameters_round_trip_defaults() {
        let opts = CheckOptions::defaults_from(LIMIT_OPTS);
        let params = build_parameters(&LIMIT_META, &opts).expect("present");
        match params.get("limit") {
            Some(ParamValue::Int(10)) => {}
            other => panic!("unexpected: {:?}", other.is_some()),
        }
    }

    #[test]
    fn layer_violation_constraint_lists_allowed_and_forbidden() {
        let cfg = fixture_layers();
        let matchers = layers::build_matchers(&cfg);
        let layer = layers::layer_for(
            &matchers,
            &cfg.project_root,
            Path::new("/repo/src/app/page.ts"),
        );
        assert_eq!(layer.as_deref(), Some("app"));

        const LV_META: CheckMeta = CheckMeta {
            id: "Design.LayerViolation",
            category: Category::Design,
            base_priority: 9,
            default_severity: Severity::High,
            explanation: "layer rationale",
            body: "",
            requires_types: false,
            consistency: false,
            options: &[],
        };
        let opts = CheckOptions::default();
        let c = build_constraint(
            Path::new("/repo/src/app/page.ts"),
            &LV_META,
            &opts,
            Severity::High,
            layer.as_deref(),
            Some(&cfg),
        );
        let allowed = c.allowed.expect("allowed populated");
        assert_eq!(allowed, vec!["domain".to_string(), "infra".to_string()]);
        let forbidden = c.forbidden.expect("forbidden populated");
        // "app" excluded (own layer), "domain" + "infra" allowed → none forbidden
        assert!(forbidden.is_empty());
    }

    #[test]
    fn orphan_export_marks_framework_entry_as_exempt() {
        let test_patterns = &[".test.", "/__tests__/"];
        let framework_patterns = &["/page.", "/layout."];
        const OE_OPTIONS: &[OptionSpec] = &[
            OptionSpec {
                name: "include_type_only",
                kind: OptionKind::Bool,
                default: OptionDefault::Bool(false),
                doc: "",
            },
            OptionSpec {
                name: "test_file_patterns",
                kind: OptionKind::StringList,
                default: OptionDefault::StringList(&[".test.", "/__tests__/"]),
                doc: "",
            },
            OptionSpec {
                name: "framework_entry_patterns",
                kind: OptionKind::StringList,
                default: OptionDefault::StringList(&["/page.", "/layout."]),
                doc: "",
            },
        ];
        const OE_META: CheckMeta = CheckMeta {
            id: "Design.OrphanExport",
            category: Category::Design,
            base_priority: 5,
            default_severity: Severity::Medium,
            explanation: "orphan rationale",
            body: "",
            requires_types: false,
            consistency: false,
            options: OE_OPTIONS,
        };
        let _ = test_patterns;
        let _ = framework_patterns;
        let opts = CheckOptions::defaults_from(OE_OPTIONS);

        let c_framework = build_constraint(
            Path::new("/repo/src/app/page.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            None,
        );
        assert_eq!(c_framework.exempt, Some(true));
        assert_eq!(c_framework.exempt_reason, Some("framework entry point"));

        let c_test = build_constraint(
            Path::new("/repo/src/foo.test.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            None,
        );
        assert_eq!(c_test.exempt, Some(true));
        assert_eq!(c_test.exempt_reason, Some("test file"));

        let c_normal = build_constraint(
            Path::new("/repo/src/domain/user.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            None,
        );
        assert_eq!(c_normal.exempt, Some(false));
    }

    #[test]
    fn split_globs_separates_patterns_from_literals() {
        let inputs = vec![
            PathBuf::from("src/foo.ts"),
            PathBuf::from("src/**/*.ts"),
            PathBuf::from("a/b"),
            PathBuf::from("[abc].ts"),
        ];
        let (literal, globs) = split_globs(&inputs);
        assert_eq!(literal.len(), 2);
        assert_eq!(globs.len(), 2);
    }
}
