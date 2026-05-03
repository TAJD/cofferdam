//! Cofferdam CLI entry point.

mod doctor;
mod explain;
mod gen_docs;
mod init;

use std::collections::HashMap;
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
use cofferdam_formatters::{
    CompactFormatter, JsonFormatter, JsonRenderOpts, TextFormatter, TextRenderOpts,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable text grouped by category (default).
    Text,
    /// Machine-readable JSON. Stable schema, no ANSI, no decorative output.
    Json,
    /// Pipe-delimited line-per-finding format. One header line followed
    /// by one record per finding. Most token-economical — use when
    /// shovelling findings into an AI prompt.
    Compact,
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
        /// Output format. Default: `text`. With `--robot` and no
        /// explicit `--format`, defaults to `json`.
        #[arg(long, value_enum, value_name = "FORMAT")]
        format: Option<OutputFormat>,
        /// Default to a machine-readable format when `--format` is not
        /// set. Token-economical output for AI agents — pairs with
        /// `--format=compact` for the smallest output by far.
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
        /// Cap rendered findings at the top N by sort priority. The CI
        /// gate (`--fail-on`) still considers the full unbounded set, so
        /// truncating output never hides a failure. Pairs with `--quiet`
        /// for compact CI output. `0` disables the cap (default).
        #[arg(long, value_name = "N", default_value_t = 0)]
        max_issues: usize,
        /// Suppress informational output: the trailing `N finding(s)`
        /// summary line, "no TypeScript files found" hints, and the
        /// "(showing N of M)" truncation note. Findings, warnings, and
        /// errors still print. Has no effect on JSON output (which is
        /// already terse).
        #[arg(long)]
        quiet: bool,
    },
    /// Manage the baseline of accepted findings. The baseline lets you
    /// drop cofferdam into an existing project without immediately
    /// failing CI on every pre-existing finding.
    Baseline {
        #[command(subcommand)]
        action: BaselineAction,
    },
    /// Print the metadata and prose explanation for one built-in check.
    /// Use this when a finding's check ID isn't self-explanatory and you
    /// want the rationale, default severity, configurable options, and
    /// any relevant flags without leaving the terminal. Add `--full` to
    /// also render the companion markdown body (motivation, examples,
    /// config snippets) sourced from the check catalog.
    Explain {
        /// Dotted check ID, e.g. `Warning.TripleEquals`. If unknown,
        /// the CLI prints the closest matches (substring on the ID) or
        /// the full list when nothing matches.
        #[arg(value_name = "CHECK_ID")]
        check_id: String,
        /// Machine-readable JSON. Schema mirrors `CheckMeta` fields.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
        /// Print the full companion markdown body after the metadata
        /// summary. In `--robot` mode, includes a `body` field in the
        /// JSON output. Frontmatter is stripped before display.
        #[arg(long)]
        full: bool,
    },
    /// Scaffold cofferdam.toml + .cofferdam/baseline.json + .gitignore
    /// entries so a new project has a working `cofferdam check` after
    /// one command. Refuses to overwrite an existing cofferdam.toml
    /// without `--force`.
    Init {
        /// Project root to initialise. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Overwrite an existing cofferdam.toml.
        #[arg(long)]
        force: bool,
        /// Capture the current set of findings as the baseline. Skip
        /// the interactive prompt.
        #[arg(long, conflicts_with = "no_baseline")]
        baseline: bool,
        /// Do not capture a baseline. Skip the interactive prompt.
        #[arg(long)]
        no_baseline: bool,
        /// Machine-readable JSON summary instead of human output. No
        /// prompts; defaults to capturing a baseline.
        #[arg(long)]
        robot: bool,
    },
    /// Diagnose install and configuration issues. Reports each check as
    /// ✓ / ⚠ / ✗ with a one-line remediation hint on failure. Exit 0 on
    /// all-pass, 1 if any check fails. Diagnostic only — never modifies
    /// files.
    Doctor {
        /// Machine-readable JSON output. Schema mirrors the per-check
        /// CheckResult and a top-level summary tally.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Apply mechanical autofixes for supported checks. Runs the engine
    /// against the given paths, groups fixable findings by file, applies
    /// edits in reverse byte-offset order, and writes each modified file
    /// atomically (write to a temp path then rename). Unsupported checks
    /// are silently skipped. Prints a summary to stderr.
    Fix {
        /// Files or directories to fix. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
    },
    /// Regenerate the docs catalog from CheckMeta. Writes per-check
    /// markdown files, a schema-stable JSON index, an llms.txt root
    /// index, and the CLI reference page (from clap-markdown). Use
    /// `--check` to fail when the committed files are out of date —
    /// same shape as `cargo fmt --check`.
    GenDocs {
        /// Output directory. The catalog lands at
        /// `<out>/checks.json`, `<out>/checks/<id>.md`,
        /// `<out>/checks/index.md`, `<out>/llms.txt`,
        /// `<out>/reference/cli.md`.
        #[arg(long, value_name = "DIR", default_value = "docs")]
        out: PathBuf,
        /// Don't write — only fail (exit 1) if the existing files
        /// would change. The CI gate uses this.
        #[arg(long)]
        check: bool,
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
            max_issues,
            quiet,
        } => run_check(CheckArgs {
            paths,
            hidden,
            no_ignore,
            format: format.unwrap_or(if robot {
                OutputFormat::Json
            } else {
                OutputFormat::Text
            }),
            pretty,
            baseline_path: baseline,
            no_baseline,
            fail_on_new,
            config_path: config,
            no_config,
            since,
            fail_on: fail_on.into(),
            max_issues,
            quiet,
        }),
        Cmd::Explain {
            check_id,
            robot,
            pretty,
            full,
        } => explain::run(explain::ExplainArgs {
            check_id,
            robot,
            pretty,
            full,
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
        Cmd::Init {
            path,
            force,
            baseline,
            no_baseline,
            robot,
        } => init::run(init::InitArgs {
            path,
            force,
            baseline_choice: if baseline {
                init::BaselineChoice::Yes
            } else if no_baseline {
                init::BaselineChoice::No
            } else {
                init::BaselineChoice::Auto
            },
            robot,
        }),
        Cmd::Doctor { robot, pretty } => doctor::run(robot, pretty),
        Cmd::Fix {
            paths,
            hidden,
            no_ignore,
        } => run_fix(paths, hidden, no_ignore),
        Cmd::GenDocs { out, check } => gen_docs::run(out, check),
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
    max_issues: usize,
    quiet: bool,
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
        max_issues,
        quiet,
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
        match format {
            OutputFormat::Json => {
                println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#)
            }
            OutputFormat::Compact => print!("{}", CompactFormatter::render(&[])),
            OutputFormat::Text if !quiet => {
                eprintln!("no TypeScript files found under: {:?}", roots);
            }
            OutputFormat::Text => {}
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
                    match format {
                        OutputFormat::Json => {
                            println!(
                                r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#
                            )
                        }
                        OutputFormat::Compact => print!("{}", CompactFormatter::render(&[])),
                        OutputFormat::Text if !quiet => {
                            eprintln!("no TypeScript files changed since {git_ref}");
                        }
                        OutputFormat::Text => {}
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
        let mut tagged: Vec<(cofferdam_core::Issue, bool)> = signed
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

        // CI gate: only NEW (un-baselined) findings at or above
        // `--fail-on` trigger exit 1. Computed from the full set BEFORE
        // truncation so `--max-issues` cannot hide a failure.
        let triggering = tagged
            .iter()
            .filter(|(issue, baselined)| !*baselined && issue.severity >= fail_on)
            .count();

        let truncated_from = apply_max_issues_tagged(&mut tagged, max_issues);
        if !quiet && format == OutputFormat::Text {
            if let Some(orig) = truncated_from {
                eprintln!("(showing {} of {} findings)", tagged.len(), orig);
            }
        }

        match format {
            OutputFormat::Text => {
                let opts = TextRenderOpts { quiet };
                print!(
                    "{}",
                    TextFormatter::render_with_baseline_opts(&tagged, opts)
                );
            }
            OutputFormat::Json => {
                let opts = JsonRenderOpts {
                    pretty,
                    truncated_from,
                };
                println!(
                    "{}",
                    JsonFormatter::render_with_baseline_with_opts(&tagged, opts)
                );
            }
            OutputFormat::Compact => {
                // Compact v1 doesn't carry baseline tags. Strip them and
                // render the underlying findings; users who need
                // baseline info should use --format=json.
                let issues_only: Vec<cofferdam_core::Issue> =
                    tagged.iter().map(|(i, _)| i.clone()).collect();
                print!("{}", CompactFormatter::render(&issues_only));
            }
        }

        if triggering == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    } else {
        match engine.analyze(&files) {
            Ok(mut issues) => {
                // CI gate: any finding at or above `--fail-on` triggers
                // exit 1. Computed from the full set BEFORE truncation
                // so `--max-issues` cannot hide a failure.
                let triggering = issues.iter().filter(|i| i.severity >= fail_on).count();

                let truncated_from = apply_max_issues(&mut issues, max_issues);
                if !quiet && format == OutputFormat::Text {
                    if let Some(orig) = truncated_from {
                        eprintln!("(showing {} of {} findings)", issues.len(), orig);
                    }
                }

                match format {
                    OutputFormat::Text => {
                        let opts = TextRenderOpts { quiet };
                        print!("{}", TextFormatter::render_with_opts(&issues, opts));
                    }
                    OutputFormat::Json => {
                        let opts = JsonRenderOpts {
                            pretty,
                            truncated_from,
                        };
                        println!("{}", JsonFormatter::render_with_opts(&issues, opts));
                    }
                    OutputFormat::Compact => {
                        print!("{}", CompactFormatter::render(&issues));
                    }
                }
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

/// Truncate `issues` to the first `max` entries (engine output is
/// already sorted by priority then check_id, so "first N" == "top N by
/// priority"). Returns `Some(original_len)` when truncation happened,
/// `None` otherwise. `max == 0` disables the cap.
fn apply_max_issues(issues: &mut Vec<cofferdam_core::Issue>, max: usize) -> Option<usize> {
    if max == 0 || issues.len() <= max {
        return None;
    }
    let original = issues.len();
    issues.truncate(max);
    Some(original)
}

fn apply_max_issues_tagged(
    tagged: &mut Vec<(cofferdam_core::Issue, bool)>,
    max: usize,
) -> Option<usize> {
    if max == 0 || tagged.len() <= max {
        return None;
    }
    let original = tagged.len();
    tagged.truncate(max);
    Some(original)
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

fn run_fix(paths: Vec<PathBuf>, hidden: bool, no_ignore: bool) -> ExitCode {
    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
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
        return ExitCode::SUCCESS;
    }

    let checks = all_builtins();

    // Build a map from check_id → &dyn Check so we can call autofix per issue.
    let check_map: HashMap<&str, &dyn cofferdam_core::Check> =
        checks.iter().map(|c| (c.meta().id, c.as_ref())).collect();

    let engine = Engine::new(all_builtins());
    let (issues, texts) = match engine.analyze_with_text(&files) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    // Collect edits per file.
    // edits_by_file: canonical path → Vec<TextEdit>
    let mut edits_by_file: HashMap<PathBuf, Vec<cofferdam_core::TextEdit>> = HashMap::new();

    for issue in &issues {
        let Some(check) = check_map.get(issue.check_id.as_str()) else {
            continue;
        };
        // Re-construct a SourceFile for the autofix call. The text was
        // cached by analyze_with_text — no additional I/O required.
        let Some(text) = texts.get(&issue.file) else {
            continue;
        };
        let source = cofferdam_core::SourceFile::new(issue.file.clone(), text.clone());
        if let Some(edit) = check.autofix(issue, &source) {
            edits_by_file
                .entry(issue.file.clone())
                .or_default()
                .push(edit);
        }
    }

    if edits_by_file.is_empty() {
        eprintln!("Applied 0 fix(es) across 0 file(s).");
        return ExitCode::SUCCESS;
    }

    let mut total_fixes: usize = 0;
    let mut total_files: usize = 0;
    let mut had_error = false;

    for (path, mut file_edits) in edits_by_file {
        let Some(original_text) = texts.get(&path) else {
            continue;
        };

        // Sort edits in REVERSE byte-offset order so applying one edit
        // doesn't shift the byte positions of earlier edits.
        file_edits.sort_by_key(|e| std::cmp::Reverse(e.span.start_byte));

        let mut text = original_text.clone();
        let mut applied = 0usize;
        for edit in &file_edits {
            let start = edit.span.start_byte as usize;
            let end = edit.span.end_byte as usize;
            if start > text.len() || end > text.len() || start > end {
                // Guard against stale or invalid spans — skip rather than panic.
                continue;
            }
            text.replace_range(start..end, &edit.replacement);
            applied += 1;
        }

        // Write atomically: write to a temp sibling, then rename.
        let tmp_path = path.with_extension("cofferdam-fix.tmp");
        if let Err(e) = std::fs::write(&tmp_path, &text) {
            eprintln!("error: could not write {}: {e}", tmp_path.display());
            had_error = true;
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            eprintln!(
                "error: could not rename {} → {}: {e}",
                tmp_path.display(),
                path.display()
            );
            // Best-effort cleanup of the temp file; ignore secondary error.
            let _ = std::fs::remove_file(&tmp_path);
            had_error = true;
            continue;
        }

        total_fixes += applied;
        total_files += 1;
    }

    eprintln!("Applied {total_fixes} fix(es) across {total_files} file(s).");

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
