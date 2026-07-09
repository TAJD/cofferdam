//! Cofferdam CLI entry point.

use cofferdam_cli::{
    advise, advise_diff, agents, baseline_diff, baseline_lint, doctor, explain, gen_docs, init,
    plugins, type_host, watch,
};

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use cofferdam_checks::all_builtins;
use cofferdam_core::Severity;
use cofferdam_engine::baseline::{self, Baseline, BaselineEntry, BaselineError};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::since;
use cofferdam_engine::{discover, DiscoveryOptions, Engine, ProjectConfig, TimingCollector};
use cofferdam_formatters::{
    CompactFormatter, JsonFormatter, JsonRenderOpts, SarifFormatter, SarifRenderOpts,
    TextFormatter, TextRenderOpts,
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
    /// SARIF 2.1.0 — OASIS-standard JSON for static-analysis tools.
    /// Upload directly to GitHub Code Scanning via
    /// `github/codeql-action/upload-sarif`, or feed Azure DevOps,
    /// GitLab, SonarQube, the VS Code Sarif Viewer, etc.
    Sarif,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum AdviseOutputFormat {
    /// Human-readable text grouped by file (default).
    Text,
    /// Machine-readable JSON array. One object per file.
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
        /// PR mode — report only findings on files changed in
        /// `<git-ref>...HEAD`. The full project tree is still analysed
        /// for cross-file soundness (OrphanExport, DeadExport, import
        /// cycles); only the reported findings are filtered to the diff.
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
        /// Hide baselined findings from text output. The summary line
        /// still reports `(N new, M baselined)` counts so the CI gate
        /// remains visible. Has no effect on `--format=json` (which
        /// always includes the per-finding `baselined` flag) or on the
        /// `--fail-on` gate (which already ignores baselined findings).
        /// Useful for routine local runs against repos with substantial
        /// baselines (cd-k23 / gh #11).
        #[arg(long)]
        hide_baselined: bool,
        /// Directory for the disk-backed findings/run cache (cd-9hp.4
        /// cp4). Defaults to `.cofferdam/cache/` under CWD. Each
        /// `cofferdam` build writes to a version-scoped subdir so an
        /// upgrade invalidates prior caches automatically. Add the
        /// directory to `.gitignore`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_cache")]
        cache_dir: Option<PathBuf>,
        /// Disable disk caching entirely. Equivalent to deleting the
        /// cache directory before each run. Cold cost only; no
        /// correctness difference.
        #[arg(long)]
        no_cache: bool,
        /// Exit with code 2 when a type-aware check is registered but
        /// the type oracle could not be started (Node unavailable,
        /// ts-morph not installed, or no tsconfig.json found). Default
        /// off: oracle failures print a warning and type-aware checks
        /// are silently skipped. Use in CI jobs that explicitly rely on
        /// type-aware coverage to catch silent regressions.
        #[arg(long)]
        fail_on_type_unavailable: bool,
        /// Print a per-check + per-phase timing breakdown to stderr
        /// (discovery, run loop, pass 2, graph build, finalize A/B, and
        /// each check's accumulated time, sorted descending). Findings
        /// output (JSON/robot/text) is byte-identical with and without
        /// this flag (CD-34).
        #[arg(long)]
        time_checks: bool,
        /// Append one `{date, category, count}` JSON row per category to
        /// `.cofferdam/trend.jsonl` (creating it if needed). Counts
        /// include baselined findings, same as `[budgets]` enforcement.
        /// Purely additive — no rendering or dashboard; pair with an
        /// external tool if you want a chart. (CD-64 D3)
        #[arg(long)]
        trend: bool,
        /// Restrict output to findings from one check (dotted id, e.g.
        /// `Warning.IslandApiConvention`). Applied after plugin merge, so
        /// it covers both built-in and plugin-emitted findings; budgets,
        /// baseline, and the exit-code gate all see only this check's
        /// findings. Useful for a CI hook that gates on a single
        /// project-specific plugin check without turning on the full
        /// suite (CD-74). An id that matches no built-in check (and no
        /// plugins are configured) exits 2 rather than silently returning
        /// zero findings — a typo here must not make a CI gate pass.
        #[arg(long, value_name = "CHECK_ID")]
        only: Option<String>,
    },
    /// Manage the baseline of accepted findings. The baseline lets you
    /// drop cofferdam into an existing project without immediately
    /// failing CI on every pre-existing finding.
    Baseline {
        #[command(subcommand)]
        action: BaselineAction,
    },
    /// Print the metadata and prose explanation for one check (built-in or
    /// plugin). Use this when a finding's check ID isn't self-explanatory and
    /// you want the rationale, default severity, configurable options, and any
    /// relevant flags without leaving the terminal. Add `--full` to also render
    /// the companion markdown body (motivation, examples, config snippets)
    /// sourced from the check catalog.
    ///
    /// Plugin checks: `explain` discovers plugin-declared checks from
    /// `cofferdam.toml`'s `plugins = [...]` and renders their `explanation`
    /// (and `body` for `--full`) the same way as built-ins. Plugins must be
    /// loadable from the current working directory; otherwise the unknown-check
    /// fallback prints suggestions only from built-ins.
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
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory until one is found or a `.git`
        /// directory is reached. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely. Plugin checks won't
        /// be resolved.
        #[arg(long)]
        no_config: bool,
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
    /// Re-analyze on file change (cd-9hp.4 cp1b). Discovers files
    /// once, registers a recursive filesystem watcher, and re-runs
    /// the engine on each detected change. A shared in-memory parse
    /// cache survives across iterations, so unchanged files skip
    /// parse on every subsequent pass. Text output only — for
    /// scripted use cases keep `cofferdam check`.
    Watch {
        /// Files or directories to watch. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories.
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Path to a `cofferdam.toml` config file. Defaults to
        /// walking up from the current directory.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely.
        #[arg(long)]
        no_config: bool,
        /// Debounce filesystem events by this many milliseconds.
        /// Lower = faster reaction to a save; higher = fewer
        /// duplicate runs when an editor emits multiple events
        /// per save. 100 ms is the typical sweet spot.
        #[arg(long, value_name = "MS", default_value_t = 100)]
        debounce: u64,
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
        /// Preview mode — discover all fixable findings and print what
        /// WOULD be changed, but do not modify any file. Exits 0. Use
        /// this to audit autofix coverage before committing to a write.
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON output. Emits a structured report
        /// instead of human-readable lines. With `--dry-run`, no files
        /// are written; the JSON describes what would change.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Print the agent-onboarding prompt — a ready-to-paste markdown block
    /// that tells an AI coding agent how to use cofferdam in this repository.
    /// Covers `advise`, `advise --diff`, `check --robot`, and the
    /// `cofferdam.invariants.toml` contract. Output is version-pinned so
    /// AGENTS.md / CLAUDE.md generators can detect staleness. Pipe into a
    /// file to create or refresh an agent context fragment:
    ///
    ///   cofferdam agents >> AGENTS.md
    Agents {
        /// Emit a paste-ready Claude Code `settings.json` hooks fragment
        /// (plus Cursor/pre-commit equivalents as comments) instead of
        /// the onboarding prompt.
        #[arg(long)]
        hooks: bool,
    },
    /// JIT architectural advisory for agents — emit the rules that apply
    /// to a given file or directory, INDEPENDENT of whether any current
    /// code violates them. Designed for agentic edit loops: an LLM agent
    /// shells out before editing a file, gets back layer membership and
    /// per-rule constraints, and adjusts its plan before writing code.
    /// Static projection — does not parse, does not run checks, does not
    /// build the project graph. With no arguments, walks the current
    /// directory.
    Advise {
        /// Files, directories, or globs to advise on. Defaults to `.`.
        /// Glob patterns (`src/**/*.ts`) work; shell expansion is
        /// honoured first, then the CLI's own globset matcher.
        paths: Vec<PathBuf>,
        /// Output format. Default: `text`. With `--robot` and no
        /// explicit `--format`, defaults to `json`.
        #[arg(long, value_enum, value_name = "FORMAT")]
        format: Option<AdviseOutputFormat>,
        /// Default to a machine-readable JSON array when `--format` is
        /// not set. Token-economical output for AI agents.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output.
        #[arg(long)]
        pretty: bool,
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory until one is found or a `.git`
        /// directory is reached. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely.
        #[arg(long)]
        no_config: bool,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Diff mode — run the engine and any configured plugins
        /// against the working tree AND the state at `<git-ref>`, then
        /// report rules that WOULD fire if the change were committed
        /// (`would_fire`) plus rules that currently fire on
        /// `<git-ref>` but are cleared by the change (`would_clear`).
        /// Output is always JSON when this flag is set; `--format` is
        /// ignored.
        #[arg(long, value_name = "GIT-REF")]
        diff: Option<String>,
        /// Severity gate for `--diff` mode. When set, the process
        /// exits 1 if any `would_fire` entry is at or above this
        /// level. `would_clear` never gates. No effect without
        /// `--diff`.
        #[arg(long, value_enum, value_name = "LEVEL")]
        fail_on: Option<FailOnLevel>,
        /// State-of-play mode (CD-65 A4): parse exactly one file (no
        /// project graph) and report `current`/`remaining` budget for
        /// the complexity/length checks (`Refactor.CyclomaticComplexity`,
        /// `Refactor.CognitiveComplexity`, `Readability.MaxFunctionLength`,
        /// `Readability.MaxLineLength`, `Design.MaxParameters`) alongside
        /// their configured `limit`. Requires exactly one path in
        /// `paths`. Always JSON; `--format`/`--diff` are ignored.
        #[arg(long)]
        analyze: bool,
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
    /// Run the Language Server Protocol server over stdio (cd-9hp.4
    /// cp5). Editors that speak LSP — VS Code (via the bundled
    /// extension stub at `editors/vscode`), Helix, neovim — connect
    /// and receive workspace diagnostics on save. The server hydrates
    /// the cp4 disk cache at startup and persists it on shutdown.
    /// Run with no arguments; the LSP transport handles its own
    /// configuration via the standard `initialize` request.
    Lsp,
    /// Type-host diagnostics (cd-9hp.2 cp1). Spawns the Node-side
    /// ts-morph worker and issues a `ping` RPC to measure cold-start
    /// timings — process spawn, ts-morph import, optional Project
    /// init. Used to verify the wire and to capture the cold-start
    /// numbers documented in `design/type-host-wire.md`.
    ///
    /// Hidden from `--help` until cp4 ships user-facing type-aware
    /// checks; meanwhile it's the only way to exercise the worker.
    #[command(hide = true)]
    TypeHost {
        /// Ping the worker (the only mode in cp1). Defaults to true.
        #[arg(long, default_value_t = true)]
        ping: bool,
        /// Project root whose `node_modules` resolves `ts-morph`.
        /// Defaults to the current directory.
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Open a ts-morph `Project` against this tsconfig and time
        /// the init. Omit to skip project init (still loads ts-morph).
        #[arg(long, value_name = "PATH")]
        tsconfig: Option<PathBuf>,
        /// Skip the `ts-morph` import entirely — measure raw Node
        /// spawn cost only. Useful when ts-morph isn't installed.
        #[arg(long)]
        no_load: bool,
        /// Machine-readable JSON output. Default is human-readable
        /// timing lines.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Lint a Typst package directory for Typst Universe submission
    /// hygiene (manifest fields, license, naming, README, bundle
    /// hygiene). Standalone from the AST engine — the unit of analysis
    /// is the package directory (`typst.toml` + `LICENSE` + `README.md`
    /// + bundle), not individual `.typ` files.
    Typst {
        /// Package directory to lint. Defaults to `.`.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output format. Default: `text`. With `--robot` and no
        /// explicit `--format`, defaults to `json`.
        #[arg(long, value_enum, value_name = "FORMAT")]
        format: Option<OutputFormat>,
        /// Default to a machine-readable format when `--format` is not
        /// set.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output (only with `--format=json` / `--robot`).
        #[arg(long)]
        pretty: bool,
        /// Severity threshold for the exit-1 gate. Findings below this
        /// level still print; the process only exits 1 if at least one
        /// finding is at this level or above.
        #[arg(long, value_enum, value_name = "LEVEL", default_value_t = FailOnLevel::Medium)]
        fail_on: FailOnLevel,
        /// Suppress the trailing `N finding(s)` summary line. Findings
        /// themselves still print. Has no effect on JSON output.
        #[arg(long)]
        quiet: bool,
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
        /// Machine-readable JSON output. Emits a `delta` block when a
        /// prior baseline existed; omitted on first run.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Report baseline entries that are also suppressed inline.
    ///
    /// A "dual-state" entry is silenced twice — once by the baseline and
    /// once by an inline directive — which wastes signal and obscures the
    /// true technical-debt posture.  Exit 0 if none found, 1 if any (so
    /// it is wireable into CI for teams that want to enforce hygiene).
    Lint {
        /// Machine-readable JSON. Schema: `{ dual_state: [...], summary: { count } }`.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Compute delta between two baselines.
    ///
    /// With two explicit paths: compares them directly.
    /// Useful for triage and PR review outside the `baseline write` path.
    Diff {
        /// First baseline path.
        #[arg(value_name = "BASELINE_A")]
        a: Option<PathBuf>,
        /// Second baseline path.
        #[arg(value_name = "BASELINE_B")]
        b: Option<PathBuf>,
        /// Machine-readable JSON.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Remove baseline entries whose signature matches no current
    /// finding (a fixed finding, a deleted file, or a renamed check).
    ///
    /// Keeps the baseline from accumulating dead weight that never gets
    /// re-examined once fixed. Default: rewrites the baseline in place.
    /// `--dry-run` lists candidates without writing. `--check` reports
    /// the same list but exits 1 if any exist and never writes — wire
    /// into CI to keep baseline hygiene from silently drifting.
    Prune {
        /// Files or directories to analyze. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Path to the baseline file. Defaults to auto-detected
        /// `.cofferdam/baseline.json`.
        #[arg(long, value_name = "PATH")]
        baseline: Option<PathBuf>,
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely.
        #[arg(long)]
        no_config: bool,
        /// List stale entries without writing. Always exits 0.
        #[arg(long, conflicts_with = "check")]
        dry_run: bool,
        /// List stale entries without writing; exit 1 if any exist.
        /// For CI gating on baseline hygiene.
        #[arg(long)]
        check: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
    },
    /// Lower `[budgets]` entries in `cofferdam.toml` to match the
    /// current finding count — never raises a budget. Run after fixing
    /// findings to lock in the improvement so a regression fails CI even
    /// if it's below the old, looser budget.
    Ratchet {
        /// Files or directories to analyze. Defaults to `.`.
        paths: Vec<PathBuf>,
        /// Walk hidden files/directories (default: skip).
        #[arg(long)]
        hidden: bool,
        /// Disable `.gitignore` / `.cofferdamignore` filtering.
        #[arg(long)]
        no_ignore: bool,
        /// Path to a `cofferdam.toml` config file. Defaults to walking
        /// up from the current directory. Conflicts with `--no-config`.
        #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
        config: Option<PathBuf>,
        /// Disable config-file discovery entirely.
        #[arg(long)]
        no_config: bool,
        /// Compute and print the new budgets without writing them.
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        robot: bool,
        /// Pretty-print JSON output. No effect without `--robot`.
        #[arg(long)]
        pretty: bool,
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
            hide_baselined,
            cache_dir,
            no_cache,
            fail_on_type_unavailable,
            time_checks,
            trend,
            only,
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
            hide_baselined,
            cache_dir,
            no_cache,
            fail_on_type_unavailable,
            time_checks,
            trend,
            only,
        }),
        Cmd::Explain {
            check_id,
            robot,
            pretty,
            full,
            config,
            no_config,
        } => explain::run(explain::ExplainArgs {
            check_id,
            robot,
            pretty,
            full,
            config_path: config,
            no_config,
        }),
        Cmd::Baseline { action } => match action {
            BaselineAction::Write {
                paths,
                hidden,
                no_ignore,
                output,
                config,
                no_config,
                robot,
                pretty,
            } => run_baseline_write(BaselineWriteArgs {
                paths,
                hidden,
                no_ignore,
                output,
                config_path: config,
                no_config,
                robot,
                pretty,
            }),
            BaselineAction::Lint { robot, pretty } => baseline_lint::run(robot, pretty),
            BaselineAction::Diff {
                a,
                b,
                robot,
                pretty,
            } => baseline_diff::run(baseline_diff::DiffArgs {
                a,
                b,
                robot,
                pretty,
            }),
            BaselineAction::Prune {
                paths,
                hidden,
                no_ignore,
                baseline,
                config,
                no_config,
                dry_run,
                check,
                robot,
                pretty,
            } => run_baseline_prune(BaselinePruneArgs {
                paths,
                hidden,
                no_ignore,
                baseline_path: baseline,
                config_path: config,
                no_config,
                dry_run,
                check,
                robot,
                pretty,
            }),
            BaselineAction::Ratchet {
                paths,
                hidden,
                no_ignore,
                config,
                no_config,
                dry_run,
                robot,
                pretty,
            } => run_baseline_ratchet(BaselineRatchetArgs {
                paths,
                hidden,
                no_ignore,
                config_path: config,
                no_config,
                dry_run,
                robot,
                pretty,
            }),
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
        Cmd::Watch {
            paths,
            hidden,
            no_ignore,
            config,
            no_config,
            debounce,
        } => watch::run(watch::WatchArgs {
            paths,
            hidden,
            no_ignore,
            config_path: config,
            no_config,
            debounce_ms: debounce,
        }),
        Cmd::Fix {
            paths,
            hidden,
            no_ignore,
            dry_run,
            robot,
            pretty,
        } => run_fix(FixArgs {
            paths,
            hidden,
            no_ignore,
            dry_run,
            robot,
            pretty,
        }),
        Cmd::Agents { hooks } => {
            if hooks {
                agents::run_hooks()
            } else {
                agents::run()
            }
        }
        Cmd::Advise {
            paths,
            format: _,
            robot: _,
            pretty,
            config,
            no_config,
            hidden: _,
            no_ignore: _,
            diff: _,
            fail_on: _,
            analyze: true,
        } => advise::run_analyze(advise::AnalyzeArgs {
            paths,
            pretty,
            config_path: config,
            no_config,
        }),
        Cmd::Advise {
            paths,
            format: _,
            robot: _,
            pretty,
            config,
            no_config,
            hidden: _,
            no_ignore: _,
            diff,
            fail_on,
            analyze: false,
        } if diff.is_some() => advise_diff::run(advise_diff::DiffArgs {
            diff_ref: diff.expect("checked by guard"),
            paths,
            fail_on: fail_on.map(Severity::from),
            config_path: config,
            no_config,
            pretty,
        }),
        Cmd::Advise {
            paths,
            format,
            robot,
            pretty,
            config,
            no_config,
            hidden,
            no_ignore,
            diff: _,
            fail_on: _,
            analyze: false,
        } => advise::run(advise::AdviseArgs {
            paths,
            format: match format.unwrap_or(if robot {
                AdviseOutputFormat::Json
            } else {
                AdviseOutputFormat::Text
            }) {
                AdviseOutputFormat::Text => advise::AdviseFormat::Text,
                AdviseOutputFormat::Json => advise::AdviseFormat::Json,
            },
            pretty,
            config_path: config,
            no_config,
            hidden,
            no_ignore,
        }),
        Cmd::GenDocs { out, check } => gen_docs::run::<Cli>(out, check),
        Cmd::Lsp => run_lsp(),
        Cmd::TypeHost {
            ping,
            project,
            tsconfig,
            no_load,
            robot,
            pretty,
        } => run_type_host_ping(TypeHostArgs {
            ping,
            project,
            tsconfig,
            no_load,
            robot,
            pretty,
        }),
        Cmd::Typst {
            path,
            format,
            robot,
            pretty,
            fail_on,
            quiet,
        } => run_typst(TypstArgs {
            path,
            format: format.unwrap_or(if robot {
                OutputFormat::Json
            } else {
                OutputFormat::Text
            }),
            pretty,
            fail_on: fail_on.into(),
            quiet,
        }),
    }
}

struct TypstArgs {
    path: PathBuf,
    format: OutputFormat,
    pretty: bool,
    fail_on: Severity,
    quiet: bool,
}

fn run_typst(args: TypstArgs) -> ExitCode {
    let TypstArgs {
        path,
        format,
        pretty,
        fail_on,
        quiet,
    } = args;

    let pkg = match cofferdam_typst::load(&path) {
        Ok(pkg) => pkg,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let mut issues: Vec<cofferdam_core::Issue> = cofferdam_typst::all_typst_checks()
        .iter()
        .flat_map(|c| c.check(&pkg))
        .collect();
    issues.sort_by(|a, b| {
        b.priority
            .0
            .cmp(&a.priority.0)
            .then_with(|| a.check_id.cmp(&b.check_id))
            .then_with(|| a.file.cmp(&b.file))
    });

    // Same gate shape as `cofferdam check`: any finding at or above
    // `--fail-on` triggers exit 1.
    let triggering = issues.iter().filter(|i| i.severity >= fail_on).count();

    match format {
        OutputFormat::Text => {
            let opts = TextRenderOpts {
                quiet,
                ..Default::default()
            };
            print!("{}", TextFormatter::render_with_opts(&issues, opts));
        }
        OutputFormat::Json => {
            // No CheckMeta catalog for Typst checks yet — omit docs_url
            // rather than emit a URL that 404s (mirrors plugin findings).
            let opts = JsonRenderOpts {
                pretty,
                truncated_from: None,
            };
            println!("{}", JsonFormatter::render_with_opts(&issues, &[], opts));
        }
        OutputFormat::Compact => {
            print!("{}", CompactFormatter::render(&issues));
        }
        OutputFormat::Sarif => {
            println!(
                "{}",
                SarifFormatter::render_with_opts(&issues, &[], SarifRenderOpts { pretty })
            );
        }
    }

    if triggering == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

struct TypeHostArgs {
    ping: bool,
    project: Option<PathBuf>,
    tsconfig: Option<PathBuf>,
    no_load: bool,
    robot: bool,
    pretty: bool,
}

fn run_type_host_ping(args: TypeHostArgs) -> ExitCode {
    if !args.ping {
        eprintln!("cofferdam type-host: only --ping mode exists in cp1");
        return ExitCode::from(2);
    }
    let project_root = args
        .project
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let open_project = args
        .tsconfig
        .as_ref()
        .map(|p| type_host::OpenProjectParams {
            tsconfig_path: p.to_string_lossy().replace('\\', "/"),
        });
    let params = type_host::PingParams {
        load_ts_morph: !args.no_load,
        open_project,
    };
    match type_host::ping(&project_root, params) {
        Ok(result) => {
            if args.robot {
                let json = if args.pretty {
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true,
                        "tsMorphVersion": result.ts_morph_version,
                        "timings": {
                            "tsMorphImportMs": result.timings.ts_morph_import_ms,
                            "projectInitMs": result.timings.project_init_ms,
                            "totalMs": result.timings.total_ms,
                        },
                    }))
                } else {
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "tsMorphVersion": result.ts_morph_version,
                        "timings": {
                            "tsMorphImportMs": result.timings.ts_morph_import_ms,
                            "projectInitMs": result.timings.project_init_ms,
                            "totalMs": result.timings.total_ms,
                        },
                    }))
                };
                println!("{}", json.unwrap_or_default());
            } else {
                println!(
                    "type-host ping ok\n  ts-morph version : {}\n  import           : {} ms\n  project init     : {} ms\n  total            : {} ms",
                    result.ts_morph_version.as_deref().unwrap_or("<not loaded>"),
                    fmt_opt_ms(result.timings.ts_morph_import_ms),
                    fmt_opt_ms(result.timings.project_init_ms),
                    result.timings.total_ms,
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if args.robot {
                let body = serde_json::json!({
                    "ok": false,
                    "error": e.to_string(),
                });
                let json = if args.pretty {
                    serde_json::to_string_pretty(&body)
                } else {
                    serde_json::to_string(&body)
                };
                println!("{}", json.unwrap_or_default());
            } else {
                eprintln!("type-host ping failed: {e}");
            }
            ExitCode::from(1)
        }
    }
}

fn fmt_opt_ms(v: Option<u64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "n/a".into(),
    }
}

/// Install a worker-backed type oracle on `engine` when (a) a registered
/// check declares `requires_types`, and (b) a `tsconfig.json` can be
/// found. Returns the engine unchanged when no type-aware check is
/// registered (the bead's auto-opt-out — no Node cost when unused) or
/// when the worker can't be started (a single warning is printed and
/// type-aware checks are skipped, rather than failing the whole run).
///
/// `project_root` anchors `tsconfig.json` discovery and `node_modules`
/// resolution for `ts-morph`.
/// Returns `(engine, unavailable_reason)`. When `unavailable_reason` is
/// `Some`, a type-aware check was registered but the oracle could not be
/// started; callers that set `--fail-on-type-unavailable` treat this as a
/// hard error (exit 2) rather than a silent skip.
fn install_type_oracle_if_needed(engine: Engine, project_root: &Path) -> (Engine, Option<String>) {
    if !engine.needs_type_oracle() {
        return (engine, None);
    }
    let Some(tsconfig) = find_tsconfig(project_root) else {
        let reason = format!("no tsconfig.json found near {}", project_root.display());
        eprintln!("warning: type-aware checks are registered but {reason} — skipping them");
        return (engine, Some(reason));
    };
    // The directory holding tsconfig.json is the natural root for
    // `node_modules` resolution; fall back to project_root.
    let node_root = tsconfig.parent().unwrap_or(project_root);
    match type_host::build_type_oracle(node_root, &tsconfig) {
        Ok(oracle) => (engine.with_type_oracle(Box::new(oracle)), None),
        Err(e) => {
            let reason = e.to_string();
            eprintln!(
                "warning: type host unavailable ({reason}) — skipping type-aware checks. \
                 Install Node + ts-morph, or set `[engine] type_aware = false` to silence this."
            );
            (engine, Some(reason))
        }
    }
}

/// Walk up from `start` looking for `tsconfig.json`. Returns the first
/// one found, or `None` at the filesystem root. Mirrors the discovery
/// the graph builder's resolver already does for `paths`/`baseUrl`.
fn find_tsconfig(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join("tsconfig.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

fn run_lsp() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match cofferdam_lsp::run_stdio_server(cwd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cofferdam-lsp: {e}");
            ExitCode::from(2)
        }
    }
}

/// Append one `{date, category, count}` JSONL row per category to
/// `.cofferdam/trend.jsonl` under the current working directory (CD-64
/// D3). Deliberately not tied to the resolved baseline path — trend
/// tracking is orthogonal to baselining and should work even with
/// `--no-baseline`.
fn append_trend(counts_by_category: &BTreeMap<String, u32>) -> std::io::Result<()> {
    use std::io::Write;
    let date = today_utc_date();
    let path = Path::new(".cofferdam/trend.jsonl");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for (category, count) in counts_by_category {
        writeln!(
            file,
            r#"{{"date":"{date}","category":"{category}","count":{count}}}"#
        )?;
    }
    Ok(())
}

/// Today's UTC calendar date as `YYYY-MM-DD`, computed from the system
/// clock without pulling in a date/time crate (the workspace has none).
fn today_utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days-since-epoch to (year, month, day). Howard Hinnant's
/// `civil_from_days` algorithm (public domain) — proleptic Gregorian,
/// valid for the full `i64` range; we only ever feed it post-1970 values.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
    hide_baselined: bool,
    cache_dir: Option<PathBuf>,
    no_cache: bool,
    fail_on_type_unavailable: bool,
    time_checks: bool,
    trend: bool,
    only: Option<String>,
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
        hide_baselined,
        cache_dir,
        no_cache,
        fail_on_type_unavailable,
        time_checks,
        trend,
        only,
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
    let timing = time_checks.then(TimingCollector::new);
    let discover_start = std::time::Instant::now();
    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(t) = timing.as_ref() {
        t.record_phase("discovery", discover_start.elapsed());
    }

    if files.is_empty() {
        match format {
            OutputFormat::Json => {
                println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#)
            }
            OutputFormat::Compact => print!("{}", CompactFormatter::render(&[])),
            OutputFormat::Sarif => {
                println!(
                    "{}",
                    SarifFormatter::render_with_opts(&[], &[], SarifRenderOpts { pretty })
                );
            }
            OutputFormat::Text if !quiet => {
                eprintln!("no TypeScript files found under: {:?}", roots);
            }
            OutputFormat::Text => {}
        }
        return ExitCode::SUCCESS;
    }

    // PR mode (`--since <ref>`): we do NOT narrow the analysis set —
    // cross-file checks (OrphanExport, DeadExport, import cycles) need the
    // full project graph to be sound. Instead we compute the set of changed
    // files and filter *findings* down to it after analysis (GH #53).
    let report_scope: Option<HashSet<PathBuf>> = match since_ref.as_deref() {
        Some(git_ref) => {
            let changed = match filter_files_since(&files, git_ref) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };
            if changed.is_empty() {
                match format {
                    OutputFormat::Json => {
                        println!(r#"{{"findings":[],"summary":{{"total":0,"by_category":{{}}}}}}"#)
                    }
                    OutputFormat::Compact => print!("{}", CompactFormatter::render(&[])),
                    OutputFormat::Sarif => {
                        println!(
                            "{}",
                            SarifFormatter::render_with_opts(&[], &[], SarifRenderOpts { pretty })
                        );
                    }
                    OutputFormat::Text if !quiet => {
                        eprintln!("no TypeScript files changed since {git_ref}");
                    }
                    OutputFormat::Text => {}
                }
                return ExitCode::SUCCESS;
            }
            Some(canonical_set(&changed))
        }
        None => None,
    };
    // NOTE: `files` is intentionally NOT reassigned. The engine receives the
    // full discovered set below.

    let baseline_loaded = match resolved_baseline.as_deref().map(load_baseline_with_warning) {
        Some(BaselineLoad::Loaded(b)) => Some(b),
        Some(BaselineLoad::Absent) => None,
        Some(BaselineLoad::VersionMismatch {
            path,
            found,
            supported,
        }) => {
            eprintln!(
                "error: baseline {} declares unsupported version {found}; this binary supports {supported}.\n\
                 Run `cofferdam baseline write` to regenerate it under the current schema.",
                path.display()
            );
            return ExitCode::from(2);
        }
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
        // When plugins are configured the unknown-check-id warning over-fires
        // — plugin-emitted IDs aren't in the built-in registry. Silence the
        // warning in that case; the host already surfaces plugin load errors
        // explicitly.
        if cfg.plugins.is_empty() {
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
    }

    let engine = match project_config.as_ref() {
        Some(cfg) => {
            // Path is only used for option-validation error context;
            // when only cofferdam.invariants.toml was loaded (no
            // cofferdam.toml), there's no per-check options bag to
            // validate, so a synthetic path is fine.
            let path = resolved_config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(all_builtins(), cfg, path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Engine::new(all_builtins()),
    };
    let project_root = project_root_for_baseline(resolved_baseline.as_deref());

    // cd-9hp.2: install the ts-morph type oracle when a registered check
    // declares `requires_types` (today: `Warning.UnusedNullCheck`). Two
    // opt-outs gate worker spawn:
    //   1. auto: `needs_type_oracle()` is false when no registered check
    //      needs types — no worker, zero added cost (cp2).
    //   2. explicit: `[engine] type_aware = false` in cofferdam.toml
    //      force-disables even when a type-aware check is registered, so
    //      CI machines without Node pay nothing and see no warning (cp4).
    // When disabled the engine has no oracle, so `requires_types` checks
    // are skipped silently in the analyze loop.
    let type_aware_enabled = project_config
        .as_ref()
        .map(cfg::ProjectConfig::type_aware_enabled)
        .unwrap_or(true);
    let type_root = project_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let (engine, type_oracle_unavailable) = if type_aware_enabled {
        install_type_oracle_if_needed(engine, &type_root)
    } else {
        (engine, None)
    };
    if fail_on_type_unavailable {
        if let Some(reason) = type_oracle_unavailable {
            eprintln!(
                "error: type host unavailable ({reason}). \
                 Remove --fail-on-type-unavailable, install Node + ts-morph, \
                 or set `[engine] type_aware = false` to opt out."
            );
            return ExitCode::from(2);
        }
    }

    // Disk-backed findings + run cache (cd-9hp.4 cp4). Resolves to
    // `Some(dir)` when caching is requested (default on); load
    // findings.json + run.json into in-memory caches before analyze,
    // save back after. `parse_cache` stays None — oxc allocators are
    // not serialisable, and the findings cache already covers most
    // of the per-file cost.
    let resolved_cache_dir: Option<PathBuf> = if no_cache {
        None
    } else {
        Some(cache_dir.unwrap_or_else(|| PathBuf::from(".cofferdam").join("cache")))
    };
    let findings_cache = cofferdam_engine::findings_cache::FindingsCache::new();
    let run_cache = cofferdam_engine::run_cache::RunCache::new();
    if let Some(dir) = resolved_cache_dir.as_deref() {
        // Map currently-registered check IDs to `&'static str` so the
        // loader can rehydrate `FindingsKey` with the static slice the
        // in-memory cache expects. Built-in IDs are always 'static
        // strings; plugin IDs aren't cached (plugins run outside the
        // engine).
        let registered_ids: Vec<&'static str> =
            all_builtins().iter().map(|c| c.meta().id).collect();
        // Load failures are silently ignored (corruption-tolerant —
        // see crate docs on `disk_cache`).
        let _ = cofferdam_engine::disk_cache::load_findings(dir, &findings_cache, &registered_ids);
        let _ = cofferdam_engine::disk_cache::load_run(dir, &run_cache);
    }

    // COMMON: analyze with signatures. Both paths use signatures here; the
    // baseline path matches them against the stored lookup, while the
    // no-baseline path discards them after plugin merging. This unifies
    // the engine call, plugin merge, and scope filter for both paths.
    let analyzed = match timing.as_ref() {
        Some(t) => engine.analyze_with_signatures_full_timed(
            &files,
            None,
            Some(&findings_cache),
            Some(&run_cache),
            t,
        ),
        None => engine.analyze_with_signatures_full(
            &files,
            None,
            Some(&findings_cache),
            Some(&run_cache),
        ),
    };
    if let Some(t) = timing.as_ref() {
        eprintln!("{}", t.report());
    }
    let mut signed = match analyzed {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    // COMMON: plugin host invocation (cd-1c7 / GH #53). Plugins run over
    // the full file set so cross-file analysis stays sound; findings are
    // report-scoped below. Both paths use the signed-pair variant so the
    // merge step is shared.
    if let Some(cfg) = project_config.as_ref() {
        signed.extend(run_plugins_filtered_with_signatures(
            cfg,
            resolved_config_path.as_deref(),
            &files,
        ));
    }
    // COMMON: report scope filter (--since). Applied after plugin merge so
    // both engine and plugin findings respect the narrowed output window.
    signed.retain(|(issue, _sig)| in_report_scope(&report_scope, &issue.file));

    // --only <CheckId> (CD-74): restrict to one check's findings, applied
    // after plugin merge so it covers plugin-emitted ids too. A check that
    // legitimately found nothing this run must NOT error — so validate
    // against the known-id registry, not against whether it fired here.
    // Plugin ids aren't statically known (same reason the cofferdam.toml
    // unknown-check-id warning above is silenced when plugins are
    // configured), so skip the typo check in that case.
    //
    // A typo hard-errors (exit 2) rather than warning-and-continuing: the
    // primary use case is a CI hook gating on one check's exit code, and a
    // silently-empty `signed` from an unmatched id would make that gate
    // pass regardless of real violations — the exact false-green failure
    // mode `--fail-on-type-unavailable` (cd-260l) and the `[budgets]` key
    // warning both exist to prevent.
    if let Some(only_id) = only.as_deref() {
        let has_plugins = project_config
            .as_ref()
            .is_some_and(|cfg| !cfg.plugins.is_empty());
        if !has_plugins && !registered.contains(&only_id) {
            eprintln!("error: --only '{only_id}' matches no known check id — check for a typo");
            return ExitCode::from(2);
        }
        signed.retain(|(issue, _sig)| issue.check_id == only_id);
    }

    // COMMON: [budgets] enforcement + --trend (CD-64 D2/D3). Both tally
    // the full finding set INCLUDING baselined findings — a budget is a
    // hard cap on debt, not a CI-gate exemption, so it must catch counts
    // a severity-based `--fail-on` gate (or a baseline) would otherwise
    // let through un-gated. Computed before truncation/baseline-tagging
    // since neither should affect the tally.
    let counts_by_check: BTreeMap<String, u32> = {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        for (issue, _sig) in &signed {
            *m.entry(issue.check_id.clone()).or_insert(0) += 1;
        }
        m
    };
    let counts_by_category: BTreeMap<String, u32> = {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        for (check_id, count) in &counts_by_check {
            let category = check_id.split('.').next().unwrap_or(check_id.as_str());
            *m.entry(category.to_string()).or_insert(0) += count;
        }
        m
    };
    // Warn on budget keys that match no known check id or category. A typo
    // (e.g. "Refactor.CognitiveComplexty") otherwise resolves to a count of 0
    // via `unwrap_or(0)` below, silently defeating the gate it was meant to be —
    // and `baseline ratchet` would then lock that dead key in at 0.
    if let Some(cfg) = project_config.as_ref() {
        if !cfg.budgets.is_empty() {
            let known: HashSet<String> = all_builtins()
                .iter()
                .flat_map(|c| {
                    let id = c.meta().id;
                    let cat = id.split('.').next().unwrap_or(id);
                    [id.to_string(), cat.to_string()]
                })
                .collect();
            for key in cfg.budgets.keys() {
                if !known.contains(key) {
                    eprintln!(
                        "warning: budget key `{key}` matches no known check id or category — it will never fire; check for a typo"
                    );
                }
            }
        }
    }
    let budget_violations: Vec<(String, u32, u32)> = project_config
        .as_ref()
        .map(|cfg| {
            cfg.budgets
                .iter()
                .filter_map(|(key, &budget)| {
                    let actual = counts_by_check
                        .get(key)
                        .or_else(|| counts_by_category.get(key))
                        .copied()
                        .unwrap_or(0);
                    (actual > budget).then_some((key.clone(), budget, actual))
                })
                .collect()
        })
        .unwrap_or_default();
    for (key, budget, actual) in &budget_violations {
        eprintln!(
            "error: budget exceeded for `{key}`: {actual} finding(s), budget is {budget} \
             (fix findings and run `cofferdam baseline ratchet` to lower the budget, or raise it deliberately)"
        );
    }
    if trend {
        if let Err(e) = append_trend(&counts_by_category) {
            eprintln!("warning: failed to append trend data: {e}");
        }
    }

    let exit_code = if baseline_active {
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
        // Re-sort after plugin merge (plugin issues appended after engine output).
        tagged.sort_by(|(a, _), (b, _)| {
            b.priority
                .0
                .cmp(&a.priority.0)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.file.cmp(&b.file))
        });

        // CI gate: only NEW (un-baselined) findings at or above
        // `--fail-on` trigger exit 1. Computed before truncation.
        let triggering = tagged
            .iter()
            .filter(|(issue, baselined)| !*baselined && issue.severity >= fail_on)
            .count();

        let truncated_from = apply_max_issues(&mut tagged, max_issues);
        print_truncation_warning(quiet, format, tagged.len(), truncated_from);

        match format {
            OutputFormat::Text => {
                let opts = TextRenderOpts {
                    quiet,
                    hide_baselined,
                };
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
                let metas: Vec<cofferdam_core::CheckMeta> =
                    all_builtins().iter().map(|c| *c.meta()).collect();
                println!(
                    "{}",
                    JsonFormatter::render_with_baseline_with_opts(&tagged, &metas, opts)
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
            OutputFormat::Sarif => {
                // SARIF v1 doesn't carry baseline tags either. Use
                // --format=json when you need the per-finding `baselined`
                // flag in the output.
                let issues_only: Vec<cofferdam_core::Issue> =
                    tagged.iter().map(|(i, _)| i.clone()).collect();
                let metas: Vec<cofferdam_core::CheckMeta> =
                    all_builtins().iter().map(|c| *c.meta()).collect();
                println!(
                    "{}",
                    SarifFormatter::render_with_opts(
                        &issues_only,
                        &metas,
                        SarifRenderOpts { pretty }
                    )
                );
            }
        }

        if triggering == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    } else {
        // Drop signatures: the no-baseline path renders plain Issues.
        let mut issues: Vec<cofferdam_core::Issue> =
            signed.into_iter().map(|(i, _sig)| i).collect();
        // Re-sort after plugin merge. Engine output is already sorted;
        // this is idempotent when no plugins were added.
        issues.sort_by(|a, b| {
            b.priority
                .0
                .cmp(&a.priority.0)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.file.cmp(&b.file))
        });

        // CI gate: any finding at or above `--fail-on` triggers exit 1.
        // Computed before truncation so `--max-issues` cannot hide a failure.
        let triggering = issues.iter().filter(|i| i.severity >= fail_on).count();

        let truncated_from = apply_max_issues(&mut issues, max_issues);
        print_truncation_warning(quiet, format, issues.len(), truncated_from);

        match format {
            OutputFormat::Text => {
                // --hide-baselined is a no-op here (nothing is baselined).
                let opts = TextRenderOpts {
                    quiet,
                    ..Default::default()
                };
                print!("{}", TextFormatter::render_with_opts(&issues, opts));
            }
            OutputFormat::Json => {
                let opts = JsonRenderOpts {
                    pretty,
                    truncated_from,
                };
                let metas: Vec<cofferdam_core::CheckMeta> =
                    all_builtins().iter().map(|c| *c.meta()).collect();
                println!("{}", JsonFormatter::render_with_opts(&issues, &metas, opts));
            }
            OutputFormat::Compact => {
                print!("{}", CompactFormatter::render(&issues));
            }
            OutputFormat::Sarif => {
                let metas: Vec<cofferdam_core::CheckMeta> =
                    all_builtins().iter().map(|c| *c.meta()).collect();
                println!(
                    "{}",
                    SarifFormatter::render_with_opts(&issues, &metas, SarifRenderOpts { pretty })
                );
            }
        }

        if triggering == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        }
    };
    // Budgets are an independent gate from `--fail-on`/baselining — a
    // budget overage fails the build even when every triggering finding
    // is baselined or below the severity threshold.
    let exit_code = if budget_violations.is_empty() {
        exit_code
    } else {
        ExitCode::from(1)
    };

    // Persist disk cache after analyze (cd-9hp.4 cp4). Save failures
    // are non-fatal warnings — a corrupted save just means the next
    // run rebuilds from cold.
    if let Some(dir) = resolved_cache_dir.as_deref() {
        if let Err(e) = cofferdam_engine::disk_cache::save_findings(dir, &findings_cache) {
            eprintln!("warning: failed to save findings cache: {e}");
        }
        if let Err(e) = cofferdam_engine::disk_cache::save_run(dir, &run_cache) {
            eprintln!("warning: failed to save run cache: {e}");
        }
    }

    exit_code
}

/// Run Node-side plugins declared in `cofferdam.toml`, then re-apply the
/// suppression directive parser to plugin findings (plugins bypass the
/// engine's built-in suppression pass since they emit out-of-band).
///
/// Returns `Vec::new()` when no plugins are configured. Errors loading
/// or running plugins surface as synthetic `Warning.Plugin*` findings
/// inside the returned vec — same shape as built-in findings — so they
/// flow through baselining, suppression, and the `--fail-on` gate.
fn run_plugins_filtered(
    cfg: &ProjectConfig,
    resolved_config_path: Option<&Path>,
    files: &[PathBuf],
) -> Vec<cofferdam_core::Issue> {
    if cfg.plugins.is_empty() {
        return Vec::new();
    }
    let cfg_dir = resolved_config_path
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let plugin_issues = plugins::run_plugins(
        &cfg.plugins,
        files,
        &cfg_dir,
        &cfg.checks,
        cfg.layers.as_ref(),
    );
    let suppression_cache: HashMap<PathBuf, cofferdam_engine::suppress::Suppressions> = files
        .iter()
        .filter_map(|p| {
            let text = std::fs::read_to_string(p).ok()?;
            Some((
                p.clone(),
                cofferdam_engine::suppress::Suppressions::parse(&text),
            ))
        })
        .collect();
    plugin_issues
        .into_iter()
        .filter(|issue| {
            let Some(sup) = suppression_cache.get(&issue.file) else {
                return true;
            };
            let bare = issue.check_id.rsplit('.').next().unwrap_or(&issue.check_id);
            !sup.is_suppressed(issue.location.line(), &issue.check_id)
                && !sup.is_suppressed(issue.location.line(), bare)
        })
        .collect()
}

/// Sister of `run_plugins_filtered` that pairs each surviving plugin
/// finding with its baseline signature. Used by the baseline-aware
/// `cofferdam check` path and by `cofferdam baseline write` so plugin
/// findings can be tagged-against / written-into `.cofferdam/baseline.json`
/// the same way engine findings are.
fn run_plugins_filtered_with_signatures(
    cfg: &ProjectConfig,
    resolved_config_path: Option<&Path>,
    files: &[PathBuf],
) -> Vec<(cofferdam_core::Issue, String)> {
    let issues = run_plugins_filtered(cfg, resolved_config_path, files);
    if issues.is_empty() {
        return Vec::new();
    }
    let mut texts: HashMap<PathBuf, String> = HashMap::new();
    issues
        .into_iter()
        .map(|issue| {
            let text = texts
                .entry(issue.file.clone())
                .or_insert_with(|| std::fs::read_to_string(&issue.file).unwrap_or_default());
            let sig = baseline::signature_for_issue(text, &issue);
            (issue, sig)
        })
        .collect()
}

/// Truncate `items` to the first `max` entries (engine output is already
/// sorted by priority then check_id, so "first N" == "top N by priority").
/// Returns `Some(original_len)` when truncation happened, `None` otherwise.
/// `max == 0` disables the cap. Generic so both `Vec<Issue>` (no-baseline
/// path) and `Vec<(Issue, bool)>` (baseline path) can share the helper.
fn apply_max_issues<T>(items: &mut Vec<T>, max: usize) -> Option<usize> {
    if max == 0 || items.len() <= max {
        return None;
    }
    let original = items.len();
    items.truncate(max);
    Some(original)
}

/// Emit the truncation warning line when output was capped by `--max-issues`.
fn print_truncation_warning(
    quiet: bool,
    format: OutputFormat,
    after_len: usize,
    truncated_from: Option<usize>,
) {
    if !quiet && format == OutputFormat::Text {
        if let Some(orig) = truncated_from {
            eprintln!("(showing {} of {} findings)", after_len, orig);
        }
    }
}

struct BaselineWriteArgs {
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    output: Option<PathBuf>,
    config_path: Option<PathBuf>,
    no_config: bool,
    robot: bool,
    pretty: bool,
}

fn run_baseline_write(args: BaselineWriteArgs) -> ExitCode {
    let BaselineWriteArgs {
        paths,
        hidden,
        no_ignore,
        output,
        config_path,
        no_config,
        robot,
        pretty,
    } = args;

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let target = output.unwrap_or_else(|| PathBuf::from(baseline::DEFAULT_PATH));

    // Read the prior baseline before overwriting (for delta computation).
    let prior_baseline: Option<baseline::Baseline> = if target.is_file() {
        baseline::read(&target).ok() // Unreadable prior — treat as first run.
    } else {
        None
    };

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
        // When plugins are configured the unknown-check-id warning over-fires
        // — plugin-emitted IDs aren't in the built-in registry. Silence the
        // warning in that case; the host already surfaces plugin load errors
        // explicitly.
        if cfg.plugins.is_empty() {
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
    }

    let engine = match project_config.as_ref() {
        Some(cfg) => {
            let path = resolved_config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(all_builtins(), cfg, path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Engine::new(all_builtins()),
    };
    let mut signed = match engine.analyze_with_signatures(&files) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    // Include plugin findings in the baseline so existing plugin-flagged
    // tech debt can be acknowledged and gradually paid down (cd-1c7).
    if let Some(cfg) = project_config.as_ref() {
        signed.extend(run_plugins_filtered_with_signatures(
            cfg,
            resolved_config_path.as_deref(),
            &files,
        ));
    }

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

    let count = baseline.findings.len();

    // Compute delta when a prior baseline was read.
    let delta = prior_baseline
        .as_ref()
        .map(|prior| baseline::compute_delta(prior, &baseline));

    if robot {
        // Machine-readable JSON output.
        let mut obj = serde_json::json!({
            "path": target.display().to_string(),
            "count": count,
        });
        if let Some(ref d) = delta {
            let by_check: serde_json::Map<String, serde_json::Value> = d
                .by_check
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        serde_json::Value::Number(serde_json::Number::from(*v)),
                    )
                })
                .collect();
            obj["delta"] = serde_json::json!({
                "added": d.added,
                "removed": d.removed,
                "by_check": by_check,
            });
        }
        let s = if pretty {
            serde_json::to_string_pretty(&obj)
        } else {
            serde_json::to_string(&obj)
        }
        .expect("serializes infallibly");
        println!("{s}");
    } else {
        eprintln!("✓ wrote {} ({} finding(s))", target.display(), count);
        if let Some(ref d) = delta {
            baseline_diff::render_delta_text(d);
        }
    }

    ExitCode::SUCCESS
}

struct BaselinePruneArgs {
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    baseline_path: Option<PathBuf>,
    config_path: Option<PathBuf>,
    no_config: bool,
    dry_run: bool,
    check: bool,
    robot: bool,
    pretty: bool,
}

/// `cofferdam baseline prune` (CD-64 D1): remove baseline entries whose
/// `(file, check_id, rule_signature)` matches no finding from a fresh
/// analysis run — a fixed finding, a deleted file, or a renamed check.
fn run_baseline_prune(args: BaselinePruneArgs) -> ExitCode {
    let BaselinePruneArgs {
        paths,
        hidden,
        no_ignore,
        baseline_path,
        config_path,
        no_config,
        dry_run,
        check,
        robot,
        pretty,
    } = args;

    let target = match baseline_path.or_else(|| resolve_baseline_path(None)) {
        Some(p) => p,
        None => {
            eprintln!(
                "error: no baseline found; run `cofferdam baseline write` first, or pass --baseline"
            );
            return ExitCode::from(2);
        }
    };
    let existing = match load_baseline_with_warning(&target) {
        BaselineLoad::Loaded(b) => b,
        BaselineLoad::Absent => {
            eprintln!("error: no baseline found at {}", target.display());
            return ExitCode::from(2);
        }
        BaselineLoad::VersionMismatch {
            path,
            found,
            supported,
        } => {
            eprintln!(
                "error: baseline {} declares unsupported version {found}; this binary supports {supported}.\n\
                 Run `cofferdam baseline write` to regenerate it under the current schema.",
                path.display()
            );
            return ExitCode::from(2);
        }
    };

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
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
    let engine = match project_config.as_ref() {
        Some(cfg) => {
            let path = resolved_config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(all_builtins(), cfg, path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Engine::new(all_builtins()),
    };
    let mut signed = match engine.analyze_with_signatures(&files) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(cfg) = project_config.as_ref() {
        signed.extend(run_plugins_filtered_with_signatures(
            cfg,
            resolved_config_path.as_deref(),
            &files,
        ));
    }

    let project_root = project_root_for_baseline(Some(&target));
    let current: HashSet<BaselineEntry> = signed
        .iter()
        .map(|(issue, sig)| baseline::entry_for(issue, sig.clone(), project_root.as_deref()))
        .collect();

    let stale: Vec<BaselineEntry> = existing
        .findings
        .iter()
        .filter(|e| !current.contains(*e))
        .cloned()
        .collect();

    if robot {
        let stale_json: Vec<_> = stale
            .iter()
            .map(|e| {
                serde_json::json!({
                    "file": e.file,
                    "check_id": e.check_id,
                    "rule_signature": e.rule_signature,
                })
            })
            .collect();
        let obj = serde_json::json!({
            "stale": stale_json,
            "summary": { "count": stale.len() },
            "wrote": !dry_run && !check && !stale.is_empty(),
        });
        let s = if pretty {
            serde_json::to_string_pretty(&obj)
        } else {
            serde_json::to_string(&obj)
        }
        .expect("serializes infallibly");
        println!("{s}");
    } else if stale.is_empty() {
        eprintln!("baseline is clean — nothing to prune");
    } else {
        for e in &stale {
            eprintln!("stale: {} {} ({})", e.file, e.check_id, e.rule_signature);
        }
        eprintln!(
            "{} stale entr{}",
            stale.len(),
            if stale.len() == 1 { "y" } else { "ies" }
        );
    }

    if check {
        return if stale.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
    }
    if dry_run || stale.is_empty() {
        return ExitCode::SUCCESS;
    }

    let stale_set: HashSet<&BaselineEntry> = stale.iter().collect();
    let kept: Vec<BaselineEntry> = existing
        .findings
        .into_iter()
        .filter(|e| !stale_set.contains(e))
        .collect();
    let pruned = Baseline::new(kept);
    if let Err(e) = baseline::write(&target, &pruned) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    if !robot {
        eprintln!(
            "✓ pruned {} entr{} → {}",
            stale.len(),
            if stale.len() == 1 { "y" } else { "ies" },
            target.display()
        );
    }
    ExitCode::SUCCESS
}

struct BaselineRatchetArgs {
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    config_path: Option<PathBuf>,
    no_config: bool,
    dry_run: bool,
    robot: bool,
    pretty: bool,
}

/// `cofferdam baseline ratchet` (CD-64 D2): lower `[budgets]` entries in
/// `cofferdam.toml` to match the current finding count for each budgeted
/// key. Never raises a budget — a key whose current count is already
/// under budget is left untouched, so ratchet can never be used to
/// quietly relax a gate.
fn run_baseline_ratchet(args: BaselineRatchetArgs) -> ExitCode {
    let BaselineRatchetArgs {
        paths,
        hidden,
        no_ignore,
        config_path,
        no_config,
        dry_run,
        robot,
        pretty,
    } = args;

    let (project_config, resolved_config_path) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };
    let (project_config, resolved_config_path) = match (project_config, resolved_config_path) {
        (Some(cfg), Some(path)) => (cfg, path),
        _ => {
            eprintln!(
                "error: no cofferdam.toml found; ratchet needs an existing [budgets] table to lower"
            );
            return ExitCode::from(2);
        }
    };
    if project_config.budgets.is_empty() {
        eprintln!(
            "no [budgets] declared in {} — nothing to ratchet",
            resolved_config_path.display()
        );
        return ExitCode::SUCCESS;
    }

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
    let engine = match Engine::with_config(all_builtins(), &project_config, &resolved_config_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut signed = match engine.analyze_with_signatures(&files) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    signed.extend(run_plugins_filtered_with_signatures(
        &project_config,
        Some(resolved_config_path.as_path()),
        &files,
    ));

    let mut counts_by_check: BTreeMap<String, u32> = BTreeMap::new();
    for (issue, _sig) in &signed {
        *counts_by_check.entry(issue.check_id.clone()).or_insert(0) += 1;
    }
    let mut counts_by_category: BTreeMap<String, u32> = BTreeMap::new();
    for (check_id, count) in &counts_by_check {
        let category = check_id.split('.').next().unwrap_or(check_id.as_str());
        *counts_by_category.entry(category.to_string()).or_insert(0) += count;
    }

    let mut lowered: Vec<(String, u32, u32)> = Vec::new();
    for (key, &budget) in &project_config.budgets {
        let actual = counts_by_check
            .get(key)
            .or_else(|| counts_by_category.get(key))
            .copied()
            .unwrap_or(0);
        if actual < budget {
            lowered.push((key.clone(), budget, actual));
        }
    }

    if robot {
        let lowered_json: Vec<_> = lowered
            .iter()
            .map(|(k, old, new)| serde_json::json!({"key": k, "from": old, "to": new}))
            .collect();
        let obj = serde_json::json!({ "lowered": lowered_json, "wrote": !dry_run && !lowered.is_empty() });
        let s = if pretty {
            serde_json::to_string_pretty(&obj)
        } else {
            serde_json::to_string(&obj)
        }
        .expect("serializes infallibly");
        println!("{s}");
    } else if lowered.is_empty() {
        eprintln!("no budget is above its current finding count — nothing to ratchet");
    } else {
        for (key, old, new) in &lowered {
            eprintln!("{key}: {old} -> {new}");
        }
    }

    if dry_run || lowered.is_empty() {
        return ExitCode::SUCCESS;
    }

    let raw = match std::fs::read_to_string(&resolved_config_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "error: failed to read {}: {e}",
                resolved_config_path.display()
            );
            return ExitCode::from(2);
        }
    };
    let mut doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "error: failed to parse {}: {e}",
                resolved_config_path.display()
            );
            return ExitCode::from(2);
        }
    };
    let budgets_table = doc
        .entry("budgets")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut();
    let Some(budgets_table) = budgets_table else {
        eprintln!(
            "error: `budgets` in {} is not a table",
            resolved_config_path.display()
        );
        return ExitCode::from(2);
    };
    for (key, _old, new) in &lowered {
        budgets_table[key.as_str()] = toml_edit::value(i64::from(*new));
    }
    if let Err(e) = std::fs::write(&resolved_config_path, doc.to_string()) {
        eprintln!(
            "error: failed to write {}: {e}",
            resolved_config_path.display()
        );
        return ExitCode::from(2);
    }
    if !robot {
        eprintln!(
            "✓ lowered {} budget(s) in {}",
            lowered.len(),
            resolved_config_path.display()
        );
    }
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
        dir = dir.parent()?;
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

/// Canonicalise a path list into a set for report-scope membership tests.
/// Mirrors `since::intersect`, which canonicalises both sides before
/// comparing, so a finding's file and a changed-file entry line up
/// regardless of relative-vs-absolute form or Windows case differences.
fn canonical_set(paths: &[PathBuf]) -> HashSet<PathBuf> {
    paths
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

/// True when `file` should appear in output. Always true outside `--since`
/// mode (scope is `None`); inside it, true only when the file's canonical
/// path is in the changed set. A file that fails to canonicalise (deleted
/// mid-run) is treated as out of scope.
fn in_report_scope(scope: &Option<HashSet<PathBuf>>, file: &Path) -> bool {
    match scope {
        None => true,
        Some(set) => std::fs::canonicalize(file)
            .map(|c| set.contains(&c))
            .unwrap_or(false),
    }
}

/// Outcome of loading a baseline for `cofferdam check`. Distinguishes a
/// schema-version mismatch (hard failure — the user's gate can't be
/// trusted) from every other read failure (soft — a missing or corrupt
/// baseline just means "run unbaselined").
enum BaselineLoad {
    Loaded(Baseline),
    Absent,
    VersionMismatch {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
}

fn load_baseline_with_warning(path: &Path) -> BaselineLoad {
    match baseline::read(path) {
        Ok(b) => BaselineLoad::Loaded(b),
        Err(BaselineError::Version {
            path,
            found,
            supported,
        }) => BaselineLoad::VersionMismatch {
            path,
            found,
            supported,
        },
        Err(e) => {
            eprintln!("warning: ignoring baseline ({e})");
            BaselineLoad::Absent
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

struct FixArgs {
    paths: Vec<PathBuf>,
    hidden: bool,
    no_ignore: bool,
    dry_run: bool,
    robot: bool,
    pretty: bool,
}

fn run_fix(args: FixArgs) -> ExitCode {
    let FixArgs {
        paths,
        hidden,
        no_ignore,
        dry_run,
        robot,
        pretty,
    } = args;

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
    // edits_by_file: canonical path → Vec<(check_id, TextEdit)>
    let mut edits_by_file: HashMap<PathBuf, Vec<(String, cofferdam_core::TextEdit)>> =
        HashMap::new();

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
                .push((issue.check_id.clone(), edit));
        }
    }

    if dry_run {
        return run_fix_dry_run(&edits_by_file, robot, pretty);
    }

    if edits_by_file.is_empty() {
        if robot {
            let summary = serde_json::json!({
                "files": [],
                "summary": { "files": 0, "edits": 0, "skipped": 0 }
            });
            let s = if pretty {
                serde_json::to_string_pretty(&summary)
            } else {
                serde_json::to_string(&summary)
            }
            .expect("summary serializes infallibly");
            println!("{s}");
        } else {
            eprintln!("Applied 0 fix(es) across 0 file(s).");
        }
        return ExitCode::SUCCESS;
    }

    let mut total_fixes: usize = 0;
    let mut total_files: usize = 0;
    let mut total_skipped: usize = 0;
    let mut had_error = false;

    for (path, file_edits) in edits_by_file {
        let Some(original_text) = texts.get(&path) else {
            continue;
        };

        // Sort edits in REVERSE byte-offset order so applying one edit
        // doesn't shift the byte positions of earlier edits.
        let mut sorted_edits: Vec<cofferdam_core::TextEdit> =
            file_edits.into_iter().map(|(_, e)| e).collect();
        sorted_edits.sort_by_key(|e| std::cmp::Reverse(e.span.start_byte));

        let mut text = original_text.clone();
        let mut applied = 0usize;
        let mut skipped = 0usize;
        for edit in &sorted_edits {
            let start = edit.span.start_byte as usize;
            let end = edit.span.end_byte as usize;
            if start > text.len() || end > text.len() || start > end {
                // Guard against stale or invalid spans — skip rather than panic.
                skipped += 1;
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
        total_skipped += skipped;
    }

    if robot {
        let summary = serde_json::json!({
            "summary": { "files": total_files, "edits": total_fixes, "skipped": total_skipped }
        });
        let s = if pretty {
            serde_json::to_string_pretty(&summary)
        } else {
            serde_json::to_string(&summary)
        }
        .expect("summary serializes infallibly");
        println!("{s}");
    } else {
        let skip_suffix = if total_skipped > 0 {
            format!(" ({total_skipped} edit(s) skipped — invalid byte span).")
        } else {
            String::new()
        };
        eprintln!("Applied {total_fixes} fix(es) across {total_files} file(s).{skip_suffix}");
    }

    if had_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Dry-run path: report what would be fixed without writing anything.
fn run_fix_dry_run(
    edits_by_file: &HashMap<PathBuf, Vec<(String, cofferdam_core::TextEdit)>>,
    robot: bool,
    pretty: bool,
) -> ExitCode {
    let total_files = edits_by_file.len();
    let total_edits: usize = edits_by_file
        .values()
        .map(|v| v.len())
        .collect::<Vec<_>>()
        .iter()
        .sum();

    if robot {
        // Build structured JSON output.
        let files: Vec<serde_json::Value> = {
            let mut sorted_paths: Vec<&PathBuf> = edits_by_file.keys().collect();
            sorted_paths.sort();
            sorted_paths
                .iter()
                .map(|path| {
                    let entries = &edits_by_file[*path];
                    // Count edits per check_id.
                    let mut by_check: std::collections::BTreeMap<&str, usize> =
                        std::collections::BTreeMap::new();
                    for (check_id, _) in entries {
                        *by_check.entry(check_id.as_str()).or_default() += 1;
                    }
                    serde_json::json!({
                        "path": path.display().to_string(),
                        "by_check": by_check,
                        "edits": entries.len(),
                    })
                })
                .collect()
        };
        let report = serde_json::json!({
            "files": files,
            "summary": { "files": total_files, "edits": total_edits },
        });
        let s = if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
        .expect("dry-run report serializes infallibly");
        println!("{s}");
    } else {
        // Human-readable dry-run output.
        if edits_by_file.is_empty() {
            println!("would touch 0 file(s), 0 autofix edit(s).");
        } else {
            let mut sorted_paths: Vec<&PathBuf> = edits_by_file.keys().collect();
            sorted_paths.sort();
            for path in &sorted_paths {
                let entries = &edits_by_file[*path];
                // Build a summary of check_id → count.
                let mut by_check: std::collections::BTreeMap<&str, usize> =
                    std::collections::BTreeMap::new();
                for (check_id, _) in entries {
                    *by_check.entry(check_id.as_str()).or_default() += 1;
                }
                let breakdown: Vec<String> = by_check
                    .iter()
                    .map(|(id, count)| format!("{id} × {count}"))
                    .collect();
                println!("would fix: {} ({})", path.display(), breakdown.join(", "));
            }
            println!(
                "would touch {} file(s), {} autofix edit(s).",
                total_files, total_edits
            );
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        // Epoch and simple offsets within the first non-leap year.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(30), (1970, 1, 31));
        assert_eq!(civil_from_days(59), (1970, 3, 1)); // 1970 is not a leap year
                                                       // First leap day of the epoch era.
        assert_eq!(civil_from_days(789), (1972, 2, 29));
        assert_eq!(civil_from_days(790), (1972, 3, 1));
        // Century divisible by 400 IS a leap year.
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
    }

    #[test]
    fn today_utc_date_is_well_formed() {
        let s = today_utc_date();
        let bytes = s.as_bytes();
        assert_eq!(s.len(), 10, "YYYY-MM-DD is 10 chars; got {s}");
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        let (y, m, d) = civil_from_days(
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                / 86_400) as i64,
        );
        assert_eq!(s, format!("{y:04}-{m:02}-{d:02}"));
        assert!((1..=12).contains(&m) && (1..=31).contains(&d));
    }
}
