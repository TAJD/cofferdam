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

    // Discover from the repo root (not cwd): `discover` returns paths
    // relative to whatever root it's given, and the ChangeSet above holds
    // absolute paths, so a cwd-relative discovery root would produce
    // discovered paths that never equal a changeset entry, silently
    // dropping every file from `out.items` (mirrors the fix already
    // applied in `run_lint_knowledge` below). Also doubles as the root
    // `render_markdown`/`render_json` relativize paths against (CD-241).
    let cwd = std::env::current_dir().expect("cwd");
    let root = knowledge::find_project_root(&cwd);

    // 2. Empty change → honest empty digest, exit 0.
    if changeset.is_empty() {
        print_digest(
            &assemble(Vec::new(), budget),
            &changeset,
            &root,
            format,
            pretty,
        );
        return ExitCode::SUCCESS;
    }

    // 3. Discover the full project — cross-file graph needs every file.
    let roots: Vec<PathBuf> = vec![root.clone()];
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
    print_digest(&digest, &changeset, &root, format, pretty);
    ExitCode::SUCCESS
}

/// `[[context_suppress]]` filtering (CD-212): drops a `ContextItem`
/// when some rule's `check_id` matches and at least one of the item's
/// `related` anchor files matches the rule's `paths` globs. An item
/// with no `related` spans can never be targeted by a path-scoped
/// rule — there's nothing to match against — but CD-227 gives a rule
/// with an empty `paths` list a dedicated wildcard meaning: suppress
/// every item this `check_id` emits, related or not, since "no globs"
/// is otherwise indistinguishable from a stale/misconfigured rule that
/// happens to match nothing.
fn suppress_items(items: Vec<ContextItem>, rules: &[cfg::ContextSuppressRule]) -> Vec<ContextItem> {
    if rules.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|item| {
            !rules.iter().any(|rule| {
                if rule.check_id != item.check_id {
                    return false;
                }
                if rule.paths.is_empty() {
                    return true;
                }
                item.related
                    .iter()
                    .any(|r| rule.is_match(&cfg::path_key(&r.file)))
            })
        })
        .collect()
}

fn print_digest(
    digest: &crate::context_digest::Digest,
    changeset: &ChangeSet,
    root: &Path,
    format: ContextFormat,
    pretty: bool,
) {
    match format {
        ContextFormat::Text => print!("{}", render_markdown(digest, changeset.files.len(), root)),
        ContextFormat::Json => println!("{}", render_json(digest, changeset, root, pretty)),
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
/// A rule with an empty `paths` list is exempt from the file-match
/// check: since CD-227 that's the deliberate "suppress every item this
/// check_id emits" wildcard, not an empty/stale glob, so there's no
/// file-match signal to validate it against. Mirrors `--lint-knowledge`'s
/// file-existence check. CD-233: every rule (wildcard or path-scoped)
/// is also validated against `all_context_providers()`'s real id set —
/// a typo'd `check_id` (`Context.Finding`, trailing whitespace, ...)
/// used to suppress nothing at runtime with no diagnostic anywhere,
/// which was the one gap CD-227's wildcard exemption otherwise left in
/// this lint for wildcard rules specifically.
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
    let known_ids: std::collections::HashSet<&str> = all_context_providers()
        .iter()
        .map(|p| p.meta().id)
        .collect();

    let mut failed = false;
    for rule in &rules {
        if !known_ids.contains(rule.check_id.as_str()) {
            eprintln!(
                "error: [[context_suppress]] rule for `{}`: not a known Context.* provider id",
                rule.check_id
            );
            failed = true;
        }
        if rule.paths.is_empty() {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// CD-237: a wildcard `[[context_suppress]]` rule (`paths` omitted)
    /// targeting `Context.Precedent` drops the CD-228/CD-235
    /// capped-groups advisory item exactly like any other
    /// `Context.Precedent` item, even though the item is `pinned: true`
    /// and carries no `related` spans. Suppression runs before
    /// `context_digest::assemble`'s pinning-aware eviction (see
    /// `run_context` step 6-7 above), so pinning offers no protection
    /// here — a user who blanket-suppresses `Context.Precedent` noise
    /// silently loses the "matching was skipped for N oversized
    /// group(s)" diagnostic along with it. Pinning current behaviour
    /// rather than asserting it should be otherwise: the docs (CD-236)
    /// call this out explicitly as the tradeoff of the wildcard form.
    #[test]
    fn wildcard_context_precedent_rule_drops_the_pinned_capped_groups_advisory_item() {
        let advisory = ContextItem {
            check_id: "Context.Precedent".to_string(),
            title: "Precedent matching skipped for 1 oversized group(s)".to_string(),
            body: "The following groups exceeded the cap...".to_string(),
            score: 0,
            pinned: true,
            related: Vec::new(),
            explain: Some("group(s) over 200 files: kinds [route.ts]".to_string()),
        };
        let rule = cfg::ContextSuppressRule {
            check_id: "Context.Precedent".to_string(),
            paths: Vec::new(),
            globset: globset::GlobSetBuilder::new()
                .build()
                .expect("empty globset"),
            root_key: "/repo".to_string(),
            reason: None,
        };

        let survivors = suppress_items(vec![advisory], &[rule]);

        assert!(
            survivors.is_empty(),
            "wildcard Context.Precedent rule must drop the pinned capped-groups advisory item; got {survivors:?}"
        );
    }

    /// Sanity check for the above: a wildcard rule for a *different*
    /// `check_id` must not touch the advisory item.
    #[test]
    fn wildcard_rule_for_a_different_check_id_leaves_the_capped_groups_advisory_item() {
        let advisory = ContextItem {
            check_id: "Context.Precedent".to_string(),
            title: "Precedent matching skipped for 1 oversized group(s)".to_string(),
            body: "The following groups exceeded the cap...".to_string(),
            score: 0,
            pinned: true,
            related: Vec::new(),
            explain: None,
        };
        let rule = cfg::ContextSuppressRule {
            check_id: "Context.Findings".to_string(),
            paths: Vec::new(),
            globset: globset::GlobSetBuilder::new()
                .build()
                .expect("empty globset"),
            root_key: "/repo".to_string(),
            reason: None,
        };

        let survivors = suppress_items(vec![advisory], &[rule]);

        assert_eq!(
            survivors.len(),
            1,
            "unrelated check_id must not suppress; got {survivors:?}"
        );
    }
}
