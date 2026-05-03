//! Cofferdam CLI entry point.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use cofferdam_checks::all_builtins;
use cofferdam_core::Severity;
use cofferdam_engine::baseline::{self, Baseline, BaselineEntry};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::since;
use cofferdam_engine::{discover, DiscoveryOptions, Engine, ProjectConfig};
use cofferdam_formatters::{JsonFormatter, TextFormatter};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable text grouped by category (default).
    Text,
    /// Machine-readable JSON. Stable schema, no ANSI, no decorative output.
    Json,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum FailOnLevel {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl From<FailOnLevel> for Severity {
    fn from(level: FailOnLevel) -> Self {
        match level {
            FailOnLevel::Info => Severity::Info,
            FailOnLevel::Low => Severity::Low,
            FailOnLevel::Medium => Severity::Medium,
            FailOnLevel::High => Severity::High,
            FailOnLevel::Critical => Severity::Critical,
        }
    }
}

const BANNER: &str = "\
  ┌─────────────────────────────────────────────────┐
  │  cofferdam v{version}                              │
  │  isolate bad code below the waterline           │
  │  measure it · sort it · ship the verdict        │
  └─────────────────────────────────────────────────┘";

#[derive(Parser)]
#[command(
    name = "cofferdam",
    version,
    about = "TypeScript code-quality analyzer"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the project banner.
    Hello,
    /// Run all checks against files or directories. With no arguments,
    /// walks the current directory.
    Check {
        /// Files or directories to analyze. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Shorthand for `--format=json`. Token-economical output for AI agents.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output (only with `--format=json` / `--robot`).
        #[arg(long)]
        pretty: bool,
        /// Path to a baseline file. Defaults to `.cofferdam/baseline.json`
        /// when that file exists. Conflicts with `--no-baseline`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_baseline")]
        baseline: Option<PathBuf>,
        /// Disable baseline detection entirely. Equivalent to running
        /// without a baseline file present.
        #[arg(long)]
        no_baseline: bool,
        /// Only fail (exit 1) on findings absent from the baseline.
        /// Implicit when a baseline is active; pass explicitly to
        /// document intent in CI scripts. Has no effect without a
        /// baseline.
        #[arg(long)]
        fail_on_new: bool,
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory until one is found or a `.git`
        /// directory is reached. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely. Equivalent to running
        /// without a `cofferdam.toml` present.
        #[arg(long)]
        no_config: bool,
        /// PR mode — only check files changed in `<git-ref>...HEAD`.
        /// Resolves the repo root via `git rev-parse --show-toplevel`
        /// and intersects discovery with the diff list. Skipped files
        /// are silently dropped from the run.
        #[arg(long, value_name = "GIT-REF")]
        since: Option<String>,
        /// Severity threshold for the exit-1 gate. Findings below this
        /// level still print; the process only exits 1 if at least one
        /// finding is at this level or above. Baselined findings never
        /// trigger the gate.
        #[arg(long, value_enum, value_name = "LEVEL", default_value_t = FailOnLevel::Medium)]
        fail_on: FailOnLevel,
    },
    /// Manage the baseline of accepted findings. The baseline lets you
    /// drop cofferdam into an existing project without immediately
    /// failing CI on every pre-existing finding.
    Baseline {
        #[command(subcommand)]
        action: BaselineAction,
    },
}

#[derive(Subcommand)]
enum BaselineAction {
    /// Run the analyzer and write the current set of findings to the
    /// baseline file. Subsequent `cofferdam check` runs ignore these
    /// findings for CI-gating purposes; they still print as
    /// `[baselined]` so the team can chip away at them.
    Write {
        /// Files or directories to analyze. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Where to write the baseline. Defaults to
        /// `.cofferdam/baseline.json` in the current directory.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory until one is found or a `.git`
        /// directory is reached. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely.
        #[arg(long)]
        no_config: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Cmd::Hello => {
            println!("{}", BANNER.replace("{version}", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Cmd::Check {
            paths,
            hidden,
            no_ignore,
            format,
            robot,
            pretty,
            baseline,
            no_baseline,
            fail_on_new,
            config,
            no_config,
            since,
            fail_on,
        } => run_check(CheckArgs {
            paths,
            hidden,
            no_ignore,
            format: if robot { OutputFormat::Json } else { format },
            pretty,
            baseline_path: baseline,
            no_baseline,
            fail_on_new,
            config_path: config,
            no_config,
            since,
            fail_on: fail_on.into(),
        }),
        Cmd::Baseline { action } => match action {
            BaselineAction::Write {
                paths,
                hidden,
                no_ignore,
                output,
                config,
                no_config,
            } => run_baseline_write(paths, hidden, no_ignore, output, config, no_config),
        },
    }
}

struct CheckArgs {
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    format: OutputFormat,
    pretty: bool,
    baseline_path: Option<PathBuf>,
    no_baseline: bool,
    fail_on_new: bool,
    config_path: Option<PathBuf>,
    no_config: bool,
    since: Option<String>,
    fail_on: Severity,
}

fn run_check(args: CheckArgs) -> ExitCode {
    let CheckArgs {
        paths,
        hidden,
        no_ignore,
        format,
        pretty,
        baseline_path,
        no_baseline,
        fail_on_new,
        config_path,
        no_config,
        since: since_ref,
        fail_on,
    } = args;

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };

    let resolved_baseline = if no_baseline {
        None
    } else {
        resolve_baseline_path(baseline_path.as_deref())
    };

    let opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };
    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    if files.is_empty() {
        if format == OutputFormat::Json {
            println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#);
        } else {
            eprintln!("no TypeScript files found under: {:?}", roots);
        }
        return ExitCode::SUCCESS;
    }

    // PR mode (`--since <ref>`): intersect discovery with `git diff
    // --name-only --diff-filter=AMR <ref>...HEAD`. Empty intersection
    // exits 0 — no changed TS files means nothing to fail on.
    let files = match since_ref.as_deref() {
        Some(git_ref) => match filter_files_since(&files, git_ref) {
            Ok(filtered) => {
                if filtered.is_empty() {
                    if format == OutputFormat::Json {
                        println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#);
                    } else {
                        eprintln!("no TypeScript files changed since {git_ref}");
                    }
                    return ExitCode::SUCCESS;
                }
                filtered
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        None => files,
    };

    let baseline_loaded = match resolved_baseline.as_deref().map(load_baseline_with_warning) {
        Some(Ok(b)) => Some(b),
        Some(Err(_)) => None,
        None => None,
    };
    let baseline_active = baseline_loaded.is_some();
    // `--fail-on-new` is documentation today — gating is governed by
    // (severity >= fail_on) AND (baselined == false). Baselined findings
    // never trigger the gate regardless of severity, so the new-only
    // semantic is implicit. Keep the flag for explicitness in CI scripts.
    let _ = fail_on_new;

    let (project_config, resolved_config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let registered: Vec<&str> = all_builtins().iter().map(|c| c.meta().id).collect();
    if let Some(cfg) = project_config.as_ref() {
        let unknown = cfg::unknown_check_ids(cfg, &registered);
        if !unknown.is_empty() {
            eprintln!(
                "warning: cofferdam.toml references unknown check id(s): {}",
                unknown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    let engine = match (project_config.as_ref(), resolved_config_path.as_deref()) {
        (Some(cfg), Some(path)) => match Engine::with_config(all_builtins(), cfg, path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        _ => Engine::new(all_builtins()),
    };
    let project_root = project_root_for_baseline(resolved_baseline.as_deref());

    if baseline_active {
        let signed = match engine.analyze_with_signatures(&files) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        };
        let lookup: HashSet<&BaselineEntry> = baseline_loaded
            .as_ref()
            .map(Baseline::lookup_set)
            .unwrap_or_default();
        let tagged: Vec<(cofferdam_core::Issue, bool)> = signed
            .into_iter()
            .map(|(issue, sig)| {
                let probe = BaselineEntry {
                    file: baseline::normalize_path(&issue.file, project_root.as_deref()),
                    check_id: issue.check_id.clone(),
                    rule_signature: sig,
                };
                let baselined = lookup.contains(&probe);
                (issue, baselined)
            })
            .collect();

        match format {
            OutputFormat::Text => print!("{}", TextFormatter::render_with_baseline(&tagged)),
            OutputFormat::Json => {
                let s = if pretty {
                    JsonFormatter::render_with_baseline_pretty(&tagged)
                } else {
                    JsonFormatter::render_with_baseline(&tagged)
                };
                println!("{}", s);
            }
        }
        // CI gate: only NEW (un-baselined) findings at or above
        // `--fail-on` trigger exit 1. Baselined findings never gate
        // regardless of severity.
        let triggering = tagged
            .iter()
            .filter(|(issue, baselined)| !*baselined && issue.severity >= fail_on)
            .count();
        if triggering == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    } else {
        match engine.analyze(&files) {
            Ok(issues) => {
                match format {
                    OutputFormat::Text => print!("{}", TextFormatter::render(&issues)),
                    OutputFormat::Json => {
                        let s = if pretty {
                            JsonFormatter::render_pretty(&issues)
                        } else {
                            JsonFormatter::render(&issues)
                        };
                        println!("{}", s);
                    }
                }
                // CI gate: any finding at or above `--fail-on` triggers
                // exit 1. With no baseline, every finding counts as new.
                let triggering = issues.iter().filter(|i| i.severity >= fail_on).count();
                if triggering == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        }
    }
}

fn run_baseline_write(
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    output: Option<PathBuf>,
    config_path: Option<PathBuf>,
    no_config: bool,
) -> ExitCode {
    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let target = output.unwrap_or_else(|| PathBuf::from(baseline::DEFAULT_PATH));

    let (project_config, resolved_config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };
    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if files.is_empty() {
        eprintln!("no TypeScript files found under: {:?}", roots);
        // Still write an empty baseline so the file exists and CI is
        // gated on "no new findings" from this point forward.
        let baseline = Baseline::new(Vec::new());
        if let Err(e) = baseline::write(&target, &baseline) {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
        eprintln!("wrote empty baseline → {}", target.display());
        return ExitCode::SUCCESS;
    }

    let registered: Vec<&str> = all_builtins().iter().map(|c| c.meta().id).collect();
    if let Some(cfg) = project_config.as_ref() {
        let unknown = cfg::unknown_check_ids(cfg, &registered);
        if !unknown.is_empty() {
            eprintln!(
                "warning: cofferdam.toml references unknown check id(s): {}",
                unknown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    let engine = match (project_config.as_ref(), resolved_config_path.as_deref()) {
        (Some(cfg), Some(path)) => match Engine::with_config(all_builtins(), cfg, path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        _ => Engine::new(all_builtins()),
    };
    let signed = match engine.analyze_with_signatures(&files) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let project_root = project_root_for_baseline(Some(&target));
    let entries: Vec<BaselineEntry> = signed
        .iter()
        .map(|(issue, sig)| baseline::entry_for(issue, sig.clone(), project_root.as_deref()))
        .collect();
    let baseline = Baseline::new(entries);
    if let Err(e) = baseline::write(&target, &baseline) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    eprintln!(
        "wrote {} finding(s) → {}",
        baseline.findings.len(),
        target.display()
    );
    ExitCode::SUCCESS
}

/// Resolve the baseline path: explicit `--baseline` wins; otherwise auto-
/// detect `.cofferdam/baseline.json` from CWD upward (walk up to the git
/// root or filesystem root). Returns `None` when nothing's found.
fn resolve_baseline_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();
    loop {
        let candidate = dir.join(baseline::DEFAULT_PATH);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}

/// The baseline file lives in `.cofferdam/baseline.json` under the
/// project root; the project root is the parent of `.cofferdam/`. Used
/// to make stored paths repository-relative so baselines diff cleanly
/// across machines.
fn project_root_for_baseline(baseline_path: Option<&Path>) -> Option<PathBuf> {
    let path = baseline_path?;
    let cofferdam_dir = path.parent()?;
    let root = cofferdam_dir.parent()?.to_path_buf();
    if root.as_os_str().is_empty() {
        std::env::current_dir().ok()
    } else {
        Some(root)
    }
}

/// Filter `files` to those changed in `<git_ref>...HEAD`. Resolves the
/// repo root via `git rev-parse --show-toplevel` from CWD; the discovery
/// list is intersected with the diff list using canonicalised paths.
fn filter_files_since(files: &[PathBuf], git_ref: &str) -> Result<Vec<PathBuf>, since::SinceError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = since::repo_root(&cwd)?;
    let changed = since::changed_files_since(&root, git_ref)?;
    Ok(since::intersect(files, &changed))
}

fn load_baseline_with_warning(path: &Path) -> Result<Baseline, ()> {
    match baseline::read(path) {
        Ok(b) => Ok(b),
        Err(e) => {
            eprintln!("warning: ignoring baseline ({e})");
            Err(())
        }
    }
}

/// Resolve which `cofferdam.toml` to load (if any) and parse it. Returns
/// `(config, path)` — both `None` when discovery is skipped or no file
/// is found. Hard-errors (return `Err(())`) only when the user passed
/// `--config <path>` to a missing or invalid file. Discovered configs
/// that fail to parse downgrade to a warning so a broken file doesn't
/// take down `cofferdam check` for users who weren't asking for config
/// in the first place.
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
                // The user pointed at this path explicitly — fail loudly
                // rather than silently ignore.
                eprintln!("error: {e}");
                Err(())
            } else {
                eprintln!("warning: ignoring config ({e})");
                Ok((None, None))
            }
        }
    }
}
