//! `cofferdam context` (CD-156, CD-157/CD-158) — resolves the current
//! diff to a `ChangeSet`, runs the engine (built-ins + Context-category
//! providers) over the full discovered project, and prints a
//! token-budgeted digest. Advisory only: always exits 0 except on
//! usage/git-resolution errors.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::context::knowledge;
use cofferdam_checks::{all_builtins, all_context_providers};
use cofferdam_core::{ChangeSet, ContextItem};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::since;
use cofferdam_engine::{discover, DiscoveryOptions, Engine, ProjectConfig};
use globset::Glob;

use crate::context_digest::{assemble, render_json, render_markdown};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFormat {
    Text,
    Json,
}

pub struct ContextArgs {
    pub paths: Vec<PathBuf>,
    pub staged: bool,
    pub base: Option<String>,
    pub budget: usize,
    pub format: Option<ContextFormat>,
    pub robot: bool,
    pub pretty: bool,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
    pub hidden: bool,
    pub no_ignore: bool,
    pub lint_knowledge: bool,
    pub lint_context_suppress: bool,
}

pub fn run(args: ContextArgs) -> ExitCode {
    let ContextArgs {
        paths,
        staged,
        base,
        budget,
        format,
        robot,
        pretty,
        config_path,
        no_config,
        hidden,
        no_ignore,
        lint_knowledge,
        lint_context_suppress,
    } = args;

    if lint_knowledge {
        return run_lint_knowledge(hidden, no_ignore, config_path.as_deref(), no_config);
    }
    if lint_context_suppress {
        return run_lint_context_suppress(hidden, no_ignore, config_path.as_deref(), no_config);
    }

    let format = format.unwrap_or(if robot {
        ContextFormat::Json
    } else {
        ContextFormat::Text
    });

    // 1. Resolve the ChangeSet.
    let changeset = if !paths.is_empty() {
        let mut abs = BTreeSet::new();
        for p in &paths {
            if !p.exists() {
                eprintln!(
                    "cofferdam context: cannot resolve {}: No such file or directory",
                    p.display()
                );
                return ExitCode::from(1);
            }
            match std::path::absolute(p) {
                Ok(c) => {
                    abs.insert(c);
                }
                Err(e) => {
                    eprintln!("cofferdam context: cannot resolve {}: {e}", p.display());
                    return ExitCode::from(1);
                }
            }
        }
        ChangeSet {
            files: abs,
            line_ranges: Default::default(),
        }
    } else {
        let cwd = std::env::current_dir().expect("cwd");
        let root = match since::repo_root(&cwd) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "cofferdam context: not in a git repository ({e}); pass explicit file paths instead"
                );
                return ExitCode::from(1);
            }
        };
        let mode = if staged {
            since::DiffMode::Staged
        } else if let Some(git_ref) = &base {
            since::DiffMode::Base(git_ref.clone())
        } else {
            since::DiffMode::WorkingTree
        };
        match since::diff_changeset(&root, &mode) {
            Ok(cs) => cs,
            Err(e) => {
                eprintln!("cofferdam context: {e}");
                return ExitCode::from(1);
            }
        }
    };

    // 2. Empty change → honest empty digest, exit 0.
    if changeset.is_empty() {
        print_digest(&assemble(Vec::new(), budget), &changeset, format, pretty);
        return ExitCode::SUCCESS;
    }

    // 3. Discover the full project — cross-file graph needs every file.
    // Discover from the repo root (not cwd): `discover` returns paths
    // relative to whatever root it's given, and the ChangeSet above holds
    // absolute paths, so a cwd-relative discovery root would produce
    // discovered paths that never equal a changeset entry, silently
    // dropping every file from `out.items` (mirrors the fix already
    // applied in `run_lint_knowledge` below).
    let cwd = std::env::current_dir().expect("cwd");
    let root = knowledge::find_project_root(&cwd);
    let roots: Vec<PathBuf> = vec![root];
    let (project_config, resolved_config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let mut opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };
    if let Some(cfg) = project_config.as_ref() {
        opts.extensions
            .extend(cfg.engine_extra_extensions.iter().cloned());
    }
    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cofferdam context: {e}");
            return ExitCode::from(2);
        }
    };

    // 4. Build the engine: all_builtins() + all_context_providers().
    let mut checks = all_builtins();
    checks.extend(all_context_providers());
    let engine = match project_config.as_ref() {
        Some(cfg) => {
            let path = resolved_config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(checks, cfg, path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("cofferdam context: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Engine::new(checks),
    };

    // 5. Read sources, run engine.analyze_context.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());
    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(text) => sources.push((path.clone(), text)),
            Err(e) => eprintln!("warning: failed to read {}: {e}", path.display()),
        }
    }

    // `out.issues` is intentionally unused in CP2 — CD-159's
    // `Context.Findings` provider will consume findings via the
    // `ALL_PRE_FILTER_FINDINGS` corpus slot, not through the CLI.
    let out = engine.analyze_context(sources, &changeset);

    // `Context.Knowledge` load-time validation warnings (CD-150 policy:
    // warn loudly, never silently match nothing) flow through as
    // ordinary `Issue`s from `finalize()` — checks may not `eprintln!`
    // directly. Surface them here since findings aren't otherwise
    // consumed by this CLI yet (CD-159's `Context.Findings` provider
    // will read them from the corpus, not from `out.issues`).
    for issue in &out.issues {
        if issue.check_id == "Context.Knowledge" {
            eprintln!("warning: {}", issue.message);
        }
    }

    // 6-7. Suppress, assemble digest, render.
    let rules = project_config
        .as_ref()
        .map(|c| c.context_suppress.as_slice())
        .unwrap_or(&[]);
    let items = suppress_items(out.items, rules);
    let digest = assemble(items, budget);
    print_digest(&digest, &changeset, format, pretty);
    ExitCode::SUCCESS
}

/// `[[context_suppress]]` filtering (CD-212): drops a `ContextItem`
/// when some rule's `check_id` matches and at least one of the item's
/// `related` anchor files matches the rule's `paths` globs. An item
/// with no `related` spans can never be suppressed by a path-scoped
/// rule — there's nothing to match against.
fn suppress_items(items: Vec<ContextItem>, rules: &[cfg::ContextSuppressRule]) -> Vec<ContextItem> {
    if rules.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|item| {
            !rules.iter().any(|rule| {
                rule.check_id == item.check_id
                    && item
                        .related
                        .iter()
                        .any(|r| rule.is_match(&cfg::path_key(&r.file)))
            })
        })
        .collect()
}

fn print_digest(
    digest: &crate::context_digest::Digest,
    changeset: &ChangeSet,
    format: ContextFormat,
    pretty: bool,
) {
    match format {
        ContextFormat::Text => print!("{}", render_markdown(digest, changeset.files.len())),
        ContextFormat::Json => println!("{}", render_json(digest, changeset, pretty)),
    }
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

/// `cofferdam context --lint-context-suppress` (CD-212): validates
/// every `[[context_suppress]]` rule instead of producing a digest.
/// A rule whose `paths` globs match zero files in the currently
/// discovered repo is almost certainly stale — the files it was
/// written to target moved, were renamed, or were deleted — per this
/// repo's "warn loudly, never silently match nothing" policy (CD-150).
/// Mirrors `--lint-knowledge`'s file-existence check; unlike
/// `--lint-knowledge` this can't validate that the rule's `check_id`
/// is a real `Context.*` provider without hardcoding the provider
/// list here, so a typo'd `check_id` is left to simply suppress
/// nothing at runtime rather than being caught by this lint.
fn run_lint_context_suppress(
    hidden: bool,
    no_ignore: bool,
    config_path: Option<&Path>,
    no_config: bool,
) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = knowledge::find_project_root(&cwd);

    let (project_config, _resolved_path) = match resolve_and_load_config(config_path, no_config) {
        Ok(pair) => pair,
        Err(()) => return ExitCode::from(2),
    };
    let rules: Vec<cfg::ContextSuppressRule> = project_config
        .as_ref()
        .map(|c| c.context_suppress.clone())
        .unwrap_or_default();

    if rules.is_empty() {
        println!(
            "cofferdam context --lint-context-suppress: no [[context_suppress]] rules declared"
        );
        return ExitCode::SUCCESS;
    }

    let mut opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };
    if let Some(cfg) = project_config.as_ref() {
        opts.extensions
            .extend(cfg.engine_extra_extensions.iter().cloned());
    }
    let files = match discover(std::slice::from_ref(&root), &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cofferdam context --lint-context-suppress: {e}");
            return ExitCode::from(2);
        }
    };
    let file_keys: Vec<String> = files.iter().map(|f| cfg::path_key(f)).collect();

    let mut failed = false;
    for rule in &rules {
        if !file_keys.iter().any(|k| rule.is_match(k)) {
            let reason = rule
                .reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            eprintln!(
                "error: [[context_suppress]] rule for `{}` (paths={:?}){reason}: matches 0 files in the current repo, likely stale",
                rule.check_id, rule.paths
            );
            failed = true;
        }
    }

    if failed {
        ExitCode::from(1)
    } else {
        println!(
            "cofferdam context --lint-context-suppress: {} rule(s) OK",
            rules.len()
        );
        ExitCode::SUCCESS
    }
}

/// `cofferdam context --lint-knowledge` (CD-162): validates every
/// `.cofferdam/knowledge/*.md` file instead of producing a digest.
/// Two failure classes, both printed as `error:` lines:
///
/// 1. Load-time validation failures (`knowledge::load_knowledge_dir`'s
///    warnings) — unparseable frontmatter, a glob/predicate that
///    failed to compile, a note left with no valid selector.
/// 2. Orphan selectors — a `match.paths` glob or `match.layers` name
///    that matches zero files in the currently discovered repo.
///    Limited to `paths`/`layers`: a `match.predicate` selector can
///    depend on import/export facts a plain file listing can't
///    evaluate without false positives, so predicate orphan-checking
///    is left to real-world usage (an always-false predicate simply
///    never contributes a digest item) rather than approximated here.
///
/// Exits nonzero on any failure — the one deliberate carve-out from
/// `cofferdam context`'s otherwise-always-exit-0 contract, so CI can
/// gate on stale knowledge notes.
fn run_lint_knowledge(
    hidden: bool,
    no_ignore: bool,
    config_path: Option<&Path>,
    no_config: bool,
) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = knowledge::find_project_root(&cwd);

    let (project_config, _resolved_path) = match resolve_and_load_config(config_path, no_config) {
        Ok(pair) => pair,
        Err(()) => return ExitCode::from(2),
    };
    let layers = project_config.as_ref().and_then(|c| c.layers.clone());

    let load = knowledge::load_knowledge_dir(&root, layers.as_ref());

    let mut failed = false;
    for (_, w) in &load.warnings {
        eprintln!("error: {w}");
        failed = true;
    }

    let mut opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };
    if let Some(cfg) = project_config.as_ref() {
        opts.extensions
            .extend(cfg.engine_extra_extensions.iter().cloned());
    }
    // Discover from `root` (not `.`): `discover` returns paths relative
    // to whatever root it's given, so walking from cwd would yield
    // `./`-prefixed paths that `strip_prefix(&root)` below can't strip
    // (root is absolute), silently defeating every selector match.
    let files = match discover(std::slice::from_ref(&root), &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cofferdam context --lint-knowledge: {e}");
            return ExitCode::from(2);
        }
    };
    let rel_files: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(&root)
                .unwrap_or(f)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    if load.notes.is_empty() {
        println!(
            "cofferdam context --lint-knowledge: no knowledge notes found under {}",
            knowledge::knowledge_dir_path(&root).display()
        );
    }

    for note in &load.notes {
        for pattern in &note.raw_paths {
            let Ok(glob) = Glob::new(pattern) else {
                // Already reported as a load-time error above.
                continue;
            };
            let matcher = glob.compile_matcher();
            if !rel_files.iter().any(|f| matcher.is_match(f)) {
                eprintln!(
                    "error: {}: orphan selector — match.paths `{pattern}` matches 0 files in the current repo",
                    note.source_path.display()
                );
                failed = true;
            }
        }
        for layer_name in &note.raw_layers {
            let matched = layers.as_ref().is_some_and(|lc| {
                lc.layers.get(layer_name).is_some_and(|globs| {
                    globs.iter().any(|pattern| {
                        Glob::new(pattern)
                            .map(|g| {
                                let m = g.compile_matcher();
                                rel_files.iter().any(|f| m.is_match(f))
                            })
                            .unwrap_or(false)
                    })
                })
            });
            if !matched {
                eprintln!(
                    "error: {}: orphan selector — match.layers `{layer_name}` matches 0 files in the current repo (or the layer isn't declared in cofferdam.invariants.toml)",
                    note.source_path.display()
                );
                failed = true;
            }
        }
    }

    if failed {
        ExitCode::from(1)
    } else {
        println!(
            "cofferdam context --lint-knowledge: {} note(s) OK",
            load.notes.len()
        );
        ExitCode::SUCCESS
    }
}
