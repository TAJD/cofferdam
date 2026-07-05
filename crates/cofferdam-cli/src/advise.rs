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
use cofferdam_core::invariants::InvariantsSpec;
use cofferdam_core::layers::{self, LayerMatcher};
use cofferdam_core::public_api::{self, PublicApi};
use cofferdam_core::{Category, CheckMeta, CheckOptions, OptionDefault, OptionValue, Severity};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::{discover, DiscoveryOptions, ProjectConfig};
use serde::Serialize;

use crate::plugins::{self, PluginCheckMeta, PluginFileScope};

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

    let advisories = match build_advisories(paths, config_path, no_config, hidden, no_ignore) {
        Ok(a) => a,
        Err(code) => return code,
    };

    match format {
        AdviseFormat::Json => emit_json(&advisories, pretty),
        AdviseFormat::Text => emit_text(&advisories),
    }

    ExitCode::SUCCESS
}

pub struct AnalyzeArgs {
    pub paths: Vec<PathBuf>,
    pub pretty: bool,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
}

/// One check's current-state reading for `advise --analyze` (CD-65 A4).
#[derive(Serialize)]
struct AnalyzeBudget {
    rule: String,
    limit: i64,
    current: u32,
    /// `limit - current`, floored at 0 for a file already over budget —
    /// negative headroom isn't meaningful to a caller deciding whether
    /// there's room left, and `check`/`baseline ratchet` are the gates
    /// that actually enforce the limit.
    remaining: i64,
}

#[derive(Serialize)]
struct AnalyzeEnvelope {
    schema_version: u32,
    file: String,
    budgets: Vec<AnalyzeBudget>,
}

/// `cofferdam advise --analyze <file>` — parse exactly one file (no
/// project graph) and report current/remaining state for the
/// checks that measure a per-function or per-file magnitude against a
/// configurable `limit`: cyclomatic/cognitive complexity, function
/// length, line width, and parameter count. Unlike the rest of `advise`,
/// this path does parse the file — it's the one place advise needs an
/// actual AST to answer "how close is this file to its limits."
pub fn run_analyze(args: AnalyzeArgs) -> ExitCode {
    let AnalyzeArgs {
        paths,
        pretty,
        config_path,
        no_config,
    } = args;

    let path = match paths.as_slice() {
        [p] => p.clone(),
        [] => {
            eprintln!("error: --analyze requires exactly one file path");
            return ExitCode::from(2);
        }
        _ => {
            eprintln!(
                "error: --analyze accepts exactly one file path, got {}",
                paths.len()
            );
            return ExitCode::from(2);
        }
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };

    let (project_config, _) = match resolve_and_load_config(config_path.as_deref(), no_config) {
        Ok(pair) => pair,
        Err(()) => return ExitCode::from(2),
    };

    let limit_of = |check_id: &str, default: i64| -> i64 {
        project_config
            .as_ref()
            .and_then(|p| {
                cfg::options_for(
                    p,
                    Path::new(""),
                    check_id,
                    &[cofferdam_core::OptionSpec {
                        name: "limit",
                        kind: cofferdam_core::OptionKind::Int,
                        default: OptionDefault::Int(default),
                        doc: "",
                    }],
                )
                .ok()
            })
            .and_then(|opts| opts.get_int("limit"))
            .unwrap_or(default)
    };

    let file = cofferdam_core::SourceFile::new(path.clone(), text);
    let allocator = cofferdam_core::Allocator::default();
    let parsed = cofferdam_core::parser::parse_into(&allocator, &file);
    let program = &parsed.program;

    let budgets = vec![
        {
            let limit = limit_of("Refactor.CyclomaticComplexity", 10);
            let current =
                cofferdam_checks::refactor::max_cyclomatic_complexity_in_file(&file, program);
            AnalyzeBudget {
                rule: "Refactor.CyclomaticComplexity".to_string(),
                limit,
                current,
                remaining: (limit - i64::from(current)).max(0),
            }
        },
        {
            let limit = limit_of("Refactor.CognitiveComplexity", 15);
            let current =
                cofferdam_checks::refactor::max_cognitive_complexity_in_file(&file, program);
            AnalyzeBudget {
                rule: "Refactor.CognitiveComplexity".to_string(),
                limit,
                current,
                remaining: (limit - i64::from(current)).max(0),
            }
        },
        {
            let limit = limit_of("Design.MaxParameters", 5);
            let current = cofferdam_checks::design::max_parameters_in_file(&file, program);
            AnalyzeBudget {
                rule: "Design.MaxParameters".to_string(),
                limit,
                current,
                remaining: (limit - i64::from(current)).max(0),
            }
        },
        {
            let limit = limit_of("Readability.MaxFunctionLength", 50);
            let current =
                cofferdam_checks::readability::max_function_length_in_file(&file, program);
            AnalyzeBudget {
                rule: "Readability.MaxFunctionLength".to_string(),
                limit,
                current,
                remaining: (limit - i64::from(current)).max(0),
            }
        },
        {
            let limit = limit_of("Readability.MaxLineLength", 120);
            let current = cofferdam_checks::readability::max_line_width_in_file(&file);
            AnalyzeBudget {
                rule: "Readability.MaxLineLength".to_string(),
                limit,
                current,
                remaining: (limit - i64::from(current)).max(0),
            }
        },
    ];

    let envelope = AnalyzeEnvelope {
        schema_version: ADVISE_SCHEMA_VERSION,
        file: path.to_string_lossy().replace('\\', "/"),
        budgets,
    };
    let s = if pretty {
        serde_json::to_string_pretty(&envelope)
    } else {
        serde_json::to_string(&envelope)
    }
    .expect("AnalyzeEnvelope serializes infallibly");
    println!("{}", s);

    ExitCode::SUCCESS
}

/// Library entry point: resolve config, discover files, and build one
/// `FileAdvisory` per path. Pure projection — used by both the CLI's
/// `run()` (which additionally formats + prints) and `cofferdam-mcp`'s
/// `cofferdam.advise` tool, so both surfaces stay byte-for-byte
/// consistent (CD-60).
pub fn build_advisories(
    paths: Vec<PathBuf>,
    config_path: Option<PathBuf>,
    no_config: bool,
    hidden: bool,
    no_ignore: bool,
) -> Result<Vec<FileAdvisory>, ExitCode> {
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
                return Err(ExitCode::from(2));
            }
        }
    };

    if !glob_specs.is_empty() {
        let walk_roots = vec![PathBuf::from(".")];
        let walked = match discover(&walk_roots, &opts) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {e}");
                return Err(ExitCode::from(2));
            }
        };
        let glob_set = match build_glob_set(&glob_specs) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("error: invalid glob pattern: {e}");
                return Err(ExitCode::from(2));
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

    let (project_config, config_path_resolved) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return Err(ExitCode::from(2)),
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

    let invariants: Option<&InvariantsSpec> =
        project_config.as_ref().and_then(|p| p.invariants.as_ref());
    let public_api_matcher: Option<PublicApi> = invariants
        .map(|spec| public_api::resolve_public_api(&spec.public_api.exports, &spec.project_root));
    let public_api_unresolved: Vec<String> = invariants
        .map(|spec| {
            spec.public_api
                .exports
                .iter()
                .filter(|e| e.starts_with("package.json:"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // Load plugin check metadata so advise can include plugin-specific
    // constraints alongside built-in ones. Metadata-mode host invocation
    // is fail-soft: if node isn't available or the host errors, plugin
    // constraints are simply omitted from the advisory output.
    let plugin_metas: Vec<PluginCheckMeta> = project_config
        .as_ref()
        .filter(|cfg| !cfg.plugins.is_empty())
        .map(|cfg| {
            let cfg_dir = config_path_resolved
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            plugins::query_plugin_metadata(&cfg.plugins, &cfg_dir)
        })
        .unwrap_or_default();

    let advise_ctx = AdviseContext {
        layers_cfg,
        layer_matchers: &layer_matchers,
        invariants,
        public_api_matcher: public_api_matcher.as_ref(),
        public_api_unresolved: &public_api_unresolved,
    };

    let advisories: Vec<FileAdvisory> = files
        .iter()
        .map(|file| build_advisory(file, &resolved, &advise_ctx, &plugin_metas))
        .collect();

    Ok(advisories)
}

/// Shared, per-run projection inputs threaded through `build_advisory` /
/// `build_constraint` for every file. Bundled into one struct to keep
/// both functions under clippy's argument-count lint.
struct AdviseContext<'a> {
    layers_cfg: Option<&'a cofferdam_core::LayersConfig>,
    layer_matchers: &'a [LayerMatcher],
    invariants: Option<&'a InvariantsSpec>,
    public_api_matcher: Option<&'a PublicApi>,
    public_api_unresolved: &'a [String],
}

fn build_advisory(
    file: &Path,
    resolved: &[(CheckMeta, CheckOptions, Severity)],
    ctx: &AdviseContext<'_>,
    plugin_metas: &[PluginCheckMeta],
) -> FileAdvisory {
    let layer = ctx
        .layers_cfg
        .and_then(|cfg| layers::layer_for(ctx.layer_matchers, &cfg.project_root, file));

    let file_abs = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let file_key = public_api::path_key(&file_abs);
    let public_api = ctx
        .public_api_matcher
        .map(|m| m.is_match(&file_key))
        .unwrap_or(false);

    let mut constraints: Vec<Constraint> = resolved
        .iter()
        .map(|(meta, options, severity)| {
            build_constraint(
                file,
                meta,
                options,
                *severity,
                layer.as_deref(),
                ctx,
                public_api,
            )
        })
        .collect();

    // Append plugin check constraints for plugin checks whose FileScope
    // matches this file. Plugin checks that don't declare a `files` scope
    // apply to every file; those with a scope are filtered here using the
    // same semantics as `fileMatchesScope` in the plugin host script.
    let fwd_path = file.to_string_lossy().replace('\\', "/");
    for pm in plugin_metas {
        if !plugin_file_matches_scope(&fwd_path, pm.files.as_ref(), layer.as_deref()) {
            continue;
        }
        let severity_str = pm.default_severity.as_str();
        let (category_s, severity_s) = (pm.category.clone(), severity_str.to_string());
        constraints.push(Constraint {
            rule: pm.id.clone(),
            category: category_s,
            severity: severity_s,
            applies: pm.explanation.clone(),
            rationale: pm.explanation.clone(),
            parameters: None,
            allowed: None,
            forbidden: None,
            exempt: None,
            exempt_reason: None,
            public_api_unresolved: None,
        });
    }

    FileAdvisory {
        path: fwd_path,
        layer,
        public_api,
        constraints,
    }
}

/// Return `true` when `fwd_path` (forward-slash normalised) is in scope
/// for the given `PluginFileScope`. Mirrors the JS `fileMatchesScope`
/// logic in plugin-host.mjs — the three (this, the .mjs, the SDK's
/// plugin-host.ts) must stay in sync. `layer` is the file's resolved
/// layer (or `None` when outside every declared layer).
fn plugin_file_matches_scope(
    fwd_path: &str,
    scope: Option<&PluginFileScope>,
    layer: Option<&str>,
) -> bool {
    let Some(scope) = scope else { return true };

    if !scope.extensions.is_empty() {
        let lower = fwd_path.to_lowercase();
        if !scope
            .extensions
            .iter()
            .any(|e| lower.ends_with(&format!(".{}", e.to_lowercase())))
        {
            return false;
        }
    }

    if !scope.layers.is_empty() {
        match layer {
            Some(l) if scope.layers.iter().any(|s| s == l) => {}
            _ => return false,
        }
    }

    let mut includes: Vec<&str> = Vec::new();
    if let Some(pp) = scope.path_pattern.as_deref() {
        if !pp.is_empty() {
            includes.push(pp);
        }
    }
    for pp in &scope.path_patterns {
        if !pp.is_empty() {
            includes.push(pp.as_str());
        }
    }

    if !includes.is_empty() && !includes.iter().any(|pat| glob_match(pat, fwd_path)) {
        return false;
    }

    if !scope.exclude_patterns.is_empty()
        && scope
            .exclude_patterns
            .iter()
            .any(|pat| glob_match(pat, fwd_path))
    {
        return false;
    }

    true
}

/// Gitignore-style glob matching (Rust mirror of `globMatch` in plugin-host.mjs).
/// Supports `**`, `*`, `?`, `[...]`, and `{a,b}` brace expansion.
/// Uses `globset` (already a workspace dep) for the per-alternative match.
///
/// Anchoring: patterns without a leading `/` or `**/` are also tried with
/// `**/` prepended so they can match anywhere in the path tree.
fn glob_match(pattern: &str, path: &str) -> bool {
    // Brace expansion: split at the first top-level `{...}`.
    if let Some((pre, rest)) = pattern.split_once('{') {
        if let Some((inner, post)) = rest.split_once('}') {
            return inner
                .split(',')
                .any(|alt| glob_match(&format!("{}{}{}", pre, alt, post), path));
        }
    }

    // For relative patterns (no leading `/` or `**/`), also try the
    // `**/pattern` form so it matches any suffix of the absolute path.
    if !pattern.starts_with('/') && !pattern.starts_with("**/") {
        let rooted = format!("**/{}", pattern);
        if glob_match_globset(&rooted, path) {
            return true;
        }
    }
    glob_match_globset(pattern, path)
}

fn glob_match_globset(pattern: &str, path: &str) -> bool {
    let gs = globset::GlobBuilder::new(pattern)
        .build()
        .and_then(|g| globset::GlobSet::builder().add(g).build());
    match gs {
        Ok(gs) => gs.is_match(path),
        Err(_) => false,
    }
}

fn build_constraint(
    file: &Path,
    meta: &CheckMeta,
    options: &CheckOptions,
    severity: Severity,
    layer: Option<&str>,
    ctx: &AdviseContext<'_>,
    public_api: bool,
) -> Constraint {
    let layers_cfg = ctx.layers_cfg;
    let invariants = ctx.invariants;
    let public_api_unresolved = ctx.public_api_unresolved;
    let parameters = build_parameters(meta, options);

    let mut allowed: Option<Vec<String>> = None;
    let mut forbidden: Option<Vec<String>> = None;
    let mut exempt: Option<bool> = None;
    let mut exempt_reason: Option<String> = None;
    let mut public_api_unresolved_field: Option<Vec<String>> = None;

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
                exempt_reason = Some("framework entry point".to_string());
            } else if matches_substring(file, &test_patterns) {
                exempt = Some(true);
                exempt_reason = Some("test file".to_string());
            } else if public_api {
                exempt = Some(true);
                exempt_reason = Some("public API entry point".to_string());
            } else {
                exempt = Some(false);
            }
            if !public_api_unresolved.is_empty() {
                public_api_unresolved_field = Some(public_api_unresolved.to_vec());
            }
            "every export must be imported somewhere in-project".to_string()
        }
        "Design.BoundaryFrozen" => match invariants {
            Some(spec) if !spec.boundaries.is_empty() => {
                let rel = relative_normalised(&spec.project_root, file);
                let matched: Vec<(&String, &cofferdam_core::invariants::BoundarySpec)> = spec
                    .boundaries
                    .iter()
                    .filter(|(glob, b)| {
                        b.frozen
                            && globset::Glob::new(glob)
                                .map(|g| g.compile_matcher().is_match(&rel))
                                .unwrap_or(false)
                    })
                    .collect();
                if matched.is_empty() {
                    "no frozen boundary covers this file".to_string()
                } else {
                    let parts: Vec<String> = matched
                        .iter()
                        .map(|(glob, b)| match b.reason.as_deref() {
                            Some(r) => format!("`{}` ({})", glob, r),
                            None => format!("`{}`", glob),
                        })
                        .collect();
                    format!("file is inside frozen boundary(ies): {}", parts.join(", "))
                }
            }
            _ => "no boundaries declared".to_string(),
        },
        "Design.InvariantViolation" => match invariants {
            Some(spec) if !spec.invariants.is_empty() => {
                let applicable: Vec<(&String, &cofferdam_core::invariants::InvariantSpec)> = spec
                    .invariants
                    .iter()
                    .filter(|(_, rule)| {
                        rule.from_layers.is_empty()
                            || layer.is_some_and(|l| rule.from_layers.iter().any(|fl| fl == l))
                    })
                    .collect();
                let mut forbid: Vec<String> = Vec::new();
                let mut require: Vec<String> = Vec::new();
                for (_, rule) in &applicable {
                    forbid.extend(rule.forbid_imports.iter().cloned());
                    require.extend(rule.require_imports.iter().cloned());
                }
                if applicable.is_empty() {
                    "no invariant rules apply to this file's layer".to_string()
                } else {
                    if !forbid.is_empty() {
                        forbidden = Some(forbid);
                    }
                    if !require.is_empty() {
                        allowed = Some(require);
                    }
                    format!(
                        "invariant(s) apply: [{}]",
                        applicable
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            _ => "no invariants declared".to_string(),
        },
        _ => default_applies(meta, options),
    };

    Constraint {
        rule: meta.id.to_string(),
        category: category_str(meta.category).to_string(),
        severity: severity.as_str().to_string(),
        applies,
        rationale: meta.explanation.to_string(),
        parameters,
        allowed,
        forbidden,
        exempt,
        exempt_reason,
        public_api_unresolved: public_api_unresolved_field,
    }
}

/// Project-root-relative, forward-slash normalised path for glob matching
/// against `[boundaries]`/`[invariants]` keys. Mirrors
/// `cofferdam-checks/src/design/mod.rs::relative_normalised` — duplicated
/// here rather than shared since it's a 6-line pure function and the two
/// crates otherwise have no reason to depend on each other's internals.
fn relative_normalised(project_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(project_root).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
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
            OptionValue::StringList(xs) => Some(format!(
                "{}=[{}]",
                spec.name,
                xs.iter()
                    .map(|s| format!("\"{}\"", s))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            OptionValue::IntList(xs) => Some(format!(
                "{}=[{}]",
                spec.name,
                xs.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
            )),
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

/// Schema version for the `advise` JSON envelope (CD-65 A5). Bump only
/// on a breaking change to `AdviseEnvelope`/`FileAdvisory`/`Constraint`'s
/// shape; new fields are additive within a major and don't need a bump.
/// Published at `/schemas/advise-v1.json` — see `docs/reference/advise.md`.
pub const ADVISE_SCHEMA_VERSION: u32 = 1;

/// Top-level JSON envelope for `cofferdam advise` (mirrors `checks.json`'s
/// `schema_version` convention). Shared by the CLI's `--format=json` and
/// `cofferdam-mcp`'s `cofferdam.advise` tool so both surfaces emit the
/// identical shape (CD-60).
#[derive(Serialize)]
pub struct AdviseEnvelope<'a> {
    pub schema_version: u32,
    pub files: &'a [FileAdvisory],
}

impl<'a> AdviseEnvelope<'a> {
    pub fn new(files: &'a [FileAdvisory]) -> Self {
        Self {
            schema_version: ADVISE_SCHEMA_VERSION,
            files,
        }
    }
}

#[derive(Serialize)]
pub struct FileAdvisory {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub public_api: bool,
    pub constraints: Vec<Constraint>,
}

#[derive(Serialize)]
pub struct Constraint {
    pub rule: String,
    pub category: String,
    pub severity: String,
    pub applies: String,
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<BTreeMap<String, ParamValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forbidden: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exempt: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exempt_reason: Option<String>,
    /// `[public_api].exports` entries this constraint could not resolve
    /// (currently `package.json:<key>` pointers — resolution deferred).
    /// Present only on `Design.OrphanExport` when such entries exist, so
    /// callers relying on `public_api`/`exempt` know the allowlist is
    /// incomplete rather than silently treating it as fully resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_api_unresolved: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    String(String),
    StringList(Vec<String>),
    IntList(Vec<i64>),
}

fn emit_json(advisories: &[FileAdvisory], pretty: bool) {
    let envelope = AdviseEnvelope::new(advisories);
    let s = if pretty {
        serde_json::to_string_pretty(&envelope)
    } else {
        serde_json::to_string(&envelope)
    }
    .expect("AdviseEnvelope serializes infallibly");
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
                    let reason = c.exempt_reason.as_deref().unwrap_or("yes");
                    let _ = writeln!(out, "      exempt: {}", reason);
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
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match cfg::resolve_with_invariants(explicit, &cwd, no_config) {
        Ok((cfg, path, diags)) => {
            for w in &diags.warnings {
                eprintln!("warning: {w}");
            }
            Ok((cfg, path))
        }
        Err(e) => {
            eprintln!("error: {e}");
            Err(())
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
        autofix: false,
        pure_run: false,
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
            autofix: false,
            pure_run: false,
        };
        let opts = CheckOptions::default();
        let ctx = AdviseContext {
            layers_cfg: Some(&cfg),
            layer_matchers: &[],
            invariants: None,
            public_api_matcher: None,
            public_api_unresolved: &[],
        };
        let c = build_constraint(
            Path::new("/repo/src/app/page.ts"),
            &LV_META,
            &opts,
            Severity::High,
            layer.as_deref(),
            &ctx,
            false,
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
            autofix: false,
            pure_run: false,
        };
        let _ = test_patterns;
        let _ = framework_patterns;
        let opts = CheckOptions::defaults_from(OE_OPTIONS);
        let ctx = AdviseContext {
            layers_cfg: None,
            layer_matchers: &[],
            invariants: None,
            public_api_matcher: None,
            public_api_unresolved: &[],
        };

        let c_framework = build_constraint(
            Path::new("/repo/src/app/page.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            &ctx,
            false,
        );
        assert_eq!(c_framework.exempt, Some(true));
        assert_eq!(
            c_framework.exempt_reason,
            Some("framework entry point".to_string())
        );

        let c_test = build_constraint(
            Path::new("/repo/src/foo.test.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            &ctx,
            false,
        );
        assert_eq!(c_test.exempt, Some(true));
        assert_eq!(c_test.exempt_reason, Some("test file".to_string()));

        let c_normal = build_constraint(
            Path::new("/repo/src/domain/user.ts"),
            &OE_META,
            &opts,
            Severity::Medium,
            None,
            &ctx,
            false,
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
