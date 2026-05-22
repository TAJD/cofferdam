//! `cofferdam advise --diff <ref>` — pre-flight validation for proposed changes.
//!
//! Runs the full check engine twice — once against pre-diff source
//! (materialised from `git show <ref>:<path>`) and once against the
//! working tree — then diffs the two issue sets keyed by `(file,
//! check_id, rule_signature)`. The signature is the same SHA-256 of
//! the trimmed offending span text used by the baseline subsystem, so
//! line shifts and reformats don't show up as spurious diff entries.
//!
//! Despite living under `advise`, this is architecturally closer to
//! `check --since` than to `advise <path>` (which is a static
//! projection that never parses anything). The shared subcommand name
//! is justified by the agent-UX framing: this is the call an agent
//! makes BEFORE committing.
//!
//! Built-in checks AND plugin findings flow through the same set-diff
//! (cd-s7f). Plugin host runs twice — once on the materialised pre
//! source and once on the working-tree post source — and its issues
//! are merged with engine issues before the diff is computed.
//!
//! V0 limitations:
//! - Single-ref form only — no `--diff a..b`. Renames counted as
//!   add+delete via `--diff-filter=AMR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Issue, Severity};
use cofferdam_engine::baseline::signature_for_span;
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::since;
use cofferdam_engine::{Engine, ProjectConfig};
use serde::Serialize;

pub(crate) struct DiffArgs {
    pub diff_ref: String,
    pub paths: Vec<PathBuf>,
    pub fail_on: Option<Severity>,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
    pub pretty: bool,
}

/// JSON identity of one would-fire / would-clear entry. The shape is a
/// trimmed `Issue` — agents care about the location, message, and
/// severity; the full priority/category bundle is recoverable via
/// `cofferdam explain <check_id>` if needed.
#[derive(Serialize, Clone)]
struct DiffIssue {
    file: String,
    check_id: String,
    severity: Severity,
    line: u32,
    column: u32,
    message: String,
}

#[derive(Serialize)]
struct DiffSummary {
    would_fire: usize,
    would_clear: usize,
}

#[derive(Serialize)]
struct DiffReport {
    #[serde(rename = "ref")]
    git_ref: String,
    would_fire: Vec<DiffIssue>,
    would_clear: Vec<DiffIssue>,
    summary: DiffSummary,
}

pub(crate) fn run(args: DiffArgs) -> ExitCode {
    let DiffArgs {
        diff_ref,
        paths,
        fail_on,
        config_path,
        no_config,
        pretty,
    } = args;

    // Resolve repo root once. Every git command we run keys off it.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let repo_root = match since::repo_root(&cwd) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    // Determine the file scope: start from the working-tree-vs-ref
    // changed set. Optional `paths` arg further restricts. We don't
    // run discovery — `git diff` already filters to tracked files,
    // and TS/TSX extension filtering happens implicitly because
    // anything else won't have any check fire on it anyway.
    let changed = match since::working_tree_changed_files(&repo_root, &diff_ref) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let scoped: Vec<PathBuf> = if paths.is_empty() {
        changed
    } else {
        // Canonicalise both sides for comparison, then keep changed
        // paths whose canonical form is under (or equal to) any
        // user-supplied path's canonical form.
        let user_canon: Vec<PathBuf> = paths
            .iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect();
        changed
            .into_iter()
            .filter(|p| {
                let Ok(c) = std::fs::canonicalize(p) else {
                    return false;
                };
                user_canon.iter().any(|u| c.starts_with(u) || c == *u)
            })
            .collect()
    };

    // Filter to TypeScript/TSX/JS sources — running checks on a
    // changed `package.json` would emit `Warning.ParseError` and
    // pollute would_fire.
    let scoped: Vec<PathBuf> = scoped
        .into_iter()
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("mts") | Some("cts")
            )
        })
        .collect();

    // Fast path: no in-scope files → empty report.
    if scoped.is_empty() {
        emit_json(
            &DiffReport {
                git_ref: diff_ref,
                would_fire: Vec::new(),
                would_clear: Vec::new(),
                summary: DiffSummary {
                    would_fire: 0,
                    would_clear: 0,
                },
            },
            pretty,
        );
        return ExitCode::SUCCESS;
    }

    // Materialise sources for both sides.
    let mut pre_sources: Vec<(PathBuf, String)> = Vec::with_capacity(scoped.len());
    let mut post_sources: Vec<(PathBuf, String)> = Vec::with_capacity(scoped.len());

    for path in &scoped {
        // Pre-source via `git show <ref>:<path>`. None means the
        // file didn't exist at <ref> — leave it absent from the pre
        // set, so every finding the engine emits on the post side
        // becomes would_fire.
        match since::read_at_ref(&repo_root, &diff_ref, path) {
            Ok(Some(text)) => pre_sources.push((path.clone(), text)),
            Ok(None) => {} // added in diff — no pre source
            Err(e) => {
                eprintln!(
                    "warning: could not read {} at {}: {e}",
                    path.display(),
                    diff_ref
                );
            }
        }
        // Post-source from disk (working tree). Skip on read errors —
        // a deleted file won't be in `--diff-filter=AMR` output, but
        // the user could have raced us.
        match std::fs::read_to_string(path) {
            Ok(text) => post_sources.push((path.clone(), text)),
            Err(e) => {
                eprintln!("warning: could not read {}: {e}", path.display());
            }
        }
    }

    // Resolve config once and use it to build both engines so
    // severity overrides and per-check options apply identically on
    // both passes.
    let (project_config, resolved_config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let make_engine = || match project_config.as_ref() {
        Some(cfg) => {
            let path = resolved_config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(all_builtins(), cfg, path) {
                Ok(e) => Some(e),
                Err(e) => {
                    eprintln!("error: {e}");
                    None
                }
            }
        }
        None => Some(Engine::new(all_builtins())),
    };

    let pre_engine = match make_engine() {
        Some(e) => e,
        None => return ExitCode::from(2),
    };
    let post_engine = match make_engine() {
        Some(e) => e,
        None => return ExitCode::from(2),
    };

    // Clone sources before the engine consumes them — plugin host
    // needs the same `(path, text)` set on each side so its findings
    // diff against the same baseline as engine findings (cd-s7f).
    let pre_for_plugins = pre_sources.clone();
    let post_for_plugins = post_sources.clone();

    let (mut pre_issues, pre_texts) = pre_engine.analyze_with_sources(pre_sources);
    let (mut post_issues, post_texts) = post_engine.analyze_with_sources(post_sources);

    // Plugin host runs on both sides so plugin findings flow through
    // the same set-diff as engine findings. Same `(plugin_paths,
    // check_options, layers_cfg)` on both invocations — only the
    // source set differs. Empty plugin list short-circuits the host
    // call inside `run_plugins_with_sources`.
    if let Some(cfg) = project_config.as_ref() {
        if !cfg.plugins.is_empty() {
            let cfg_dir = resolved_config_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let pre_plugin_issues = crate::plugins::run_plugins_with_sources(
                &cfg.plugins,
                &pre_for_plugins,
                &cfg_dir,
                &cfg.checks,
                cfg.layers.as_ref(),
            );
            let post_plugin_issues = crate::plugins::run_plugins_with_sources(
                &cfg.plugins,
                &post_for_plugins,
                &cfg_dir,
                &cfg.checks,
                cfg.layers.as_ref(),
            );
            pre_issues.extend(pre_plugin_issues);
            post_issues.extend(post_plugin_issues);
        }
    }

    // Build keyed maps for set-diff. Key = (normalized file path,
    // check_id, rule_signature). Path is normalized to forward-slash
    // relative-to-repo-root so that any future cross-platform
    // sensitivity is contained.
    let pre_map = build_keyed_map(&pre_issues, &pre_texts, &repo_root);
    let post_map = build_keyed_map(&post_issues, &post_texts, &repo_root);

    let mut would_fire: Vec<DiffIssue> = post_map
        .iter()
        .filter(|(k, _)| !pre_map.contains_key(*k))
        .map(|(_, v)| v.clone())
        .collect();
    let mut would_clear: Vec<DiffIssue> = pre_map
        .iter()
        .filter(|(k, _)| !post_map.contains_key(*k))
        .map(|(_, v)| v.clone())
        .collect();

    sort_diff_issues(&mut would_fire);
    sort_diff_issues(&mut would_clear);

    let summary = DiffSummary {
        would_fire: would_fire.len(),
        would_clear: would_clear.len(),
    };

    let report = DiffReport {
        git_ref: diff_ref,
        would_fire,
        would_clear,
        summary,
    };

    // CI gate. `--fail-on=<level>` exits 1 when any would_fire entry
    // is at or above the given severity. would_clear never gates —
    // regression-cleared signal should never block CI.
    let exit = if let Some(threshold) = fail_on {
        let any_at_or_above = report
            .would_fire
            .iter()
            .any(|i| severity_ge(i.severity, threshold));
        if any_at_or_above {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    } else {
        ExitCode::SUCCESS
    };

    emit_json(&report, pretty);
    exit
}

fn build_keyed_map(
    issues: &[Issue],
    texts: &std::collections::HashMap<PathBuf, String>,
    repo_root: &Path,
) -> BTreeMap<(String, String, String), DiffIssue> {
    let empty = String::new();
    issues
        .iter()
        .map(|issue| {
            let text = texts.get(&issue.file).unwrap_or(&empty);
            let sig = signature_for_span(text, &issue.location);
            let file = normalize_for_key(&issue.file, repo_root);
            let key = (file.clone(), issue.check_id.clone(), sig);
            let value = DiffIssue {
                file,
                check_id: issue.check_id.clone(),
                severity: issue.severity,
                line: issue.location.line(),
                column: issue.location.column(),
                message: issue.message.clone(),
            };
            (key, value)
        })
        .collect()
}

fn normalize_for_key(path: &Path, repo_root: &Path) -> String {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn sort_diff_issues(items: &mut [DiffIssue]) {
    items.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.check_id.cmp(&b.check_id))
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.column.cmp(&b.column))
    });
}

fn severity_ge(a: Severity, b: Severity) -> bool {
    a >= b
}

fn emit_json(report: &DiffReport, pretty: bool) {
    let s = if pretty {
        serde_json::to_string_pretty(report)
    } else {
        serde_json::to_string(report)
    }
    .expect("DiffReport serializes infallibly");
    println!("{s}");
}

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
