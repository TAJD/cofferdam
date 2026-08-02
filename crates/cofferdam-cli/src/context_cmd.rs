//! `cofferdam context` (CD-156, CD-157/CD-158) — resolves the current
//! diff to a `ChangeSet`, runs the engine (built-ins + Context-category
//! providers) over the full discovered project, and prints a
//! token-budgeted digest. Advisory only: always exits 0 except on
//! usage/git-resolution errors.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::{all_builtins, all_context_providers};
use cofferdam_core::ChangeSet;
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::since;
use cofferdam_engine::{discover, DiscoveryOptions, Engine, ProjectConfig};

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
    } = args;

    let format = format.unwrap_or(if robot {
        ContextFormat::Json
    } else {
        ContextFormat::Text
    });

    // 1. Resolve the ChangeSet.
    let changeset = if !paths.is_empty() {
        let mut abs = BTreeSet::new();
        for p in &paths {
            match std::fs::canonicalize(p) {
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
    let roots: Vec<PathBuf> = vec![PathBuf::from(".")];
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

    // 6-7. Assemble digest and render.
    let digest = assemble(out.items, budget);
    print_digest(&digest, &changeset, format, pretty);
    ExitCode::SUCCESS
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
