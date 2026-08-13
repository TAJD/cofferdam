//! `cofferdam doctor` — diagnose install and configuration issues.
//!
//! Runs 11 checks covering binary integrity, config, baseline, baseline
//! suppression overlap, git, discovery, suppression directives, type-host
//! tsconfig availability, npm wrapper version, and biome/eslint
//! coexistence. Reports every check in one pass (no early exit);
//! returns `ExitCode::FAILURE` if any check has `Status::Fail`,
//! `ExitCode::SUCCESS` otherwise (warns alone do not cause non-zero exit).
//!
//! This command is **diagnostic only** — it never modifies files.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::invariants;
use cofferdam_engine::baseline::{self, Baseline, DEFAULT_PATH as BASELINE_DEFAULT_PATH};
use cofferdam_engine::config::{self as cfg};
use cofferdam_engine::suppress;
use cofferdam_engine::{discover, DiscoveryOptions};
use serde::Serialize;

use crate::baseline_lint;

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    /// Check was skipped (e.g. wrapper-version outside an npm install).
    Skipped,
}

impl Status {
    fn icon(&self) -> &'static str {
        match self {
            Status::Pass => "✓",
            Status::Warn => "⚠",
            Status::Fail => "✗",
            Status::Skipped => "-",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: Status,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl CheckResult {
    fn pass(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Pass,
            message: message.into(),
            remediation: None,
        }
    }

    fn warn(
        name: &'static str,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            name,
            status: Status::Warn,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }

    fn fail(
        name: &'static str,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            name,
            status: Status::Fail,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }

    fn skipped(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Skipped,
            message: message.into(),
            remediation: None,
        }
    }
}

#[derive(Serialize)]
struct DoctorSummary {
    pass: usize,
    warn: usize,
    fail: usize,
    skipped: usize,
}

#[derive(Serialize)]
struct DoctorReport {
    results: Vec<CheckResult>,
    summary: DoctorSummary,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Run all diagnostic checks and emit either text or JSON output.
///
/// Returns `ExitCode::FAILURE` when any check has `Status::Fail`.
/// Warnings alone do **not** cause a non-zero exit.
pub fn run(robot: bool, pretty: bool) -> ExitCode {
    let results = vec![
        check_binary_integrity(),
        check_config(),
        check_invariants_layers(),
        check_baseline(),
        check_baseline_suppression(),
        check_git(),
        check_discovery(),
        check_suppression_directives(),
        check_type_host(),
        check_wrapper_version(),
        check_formatter_coexistence(),
    ];

    let summary = DoctorSummary {
        pass: results.iter().filter(|r| r.status == Status::Pass).count(),
        warn: results.iter().filter(|r| r.status == Status::Warn).count(),
        fail: results.iter().filter(|r| r.status == Status::Fail).count(),
        skipped: results
            .iter()
            .filter(|r| r.status == Status::Skipped)
            .count(),
    };

    let has_failure = summary.fail > 0;

    if robot {
        let report = DoctorReport { results, summary };
        let s = if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
        .expect("DoctorReport serializes infallibly");
        println!("{s}");
    } else {
        render_text(&results, &summary);
    }

    if has_failure {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ── Text renderer ─────────────────────────────────────────────────────────────

fn render_text(results: &[CheckResult], summary: &DoctorSummary) {
    // Compute the longest name for alignment.
    let max_name = results.iter().map(|r| r.name.len()).max().unwrap_or(12);

    for r in results {
        let padding = " ".repeat(max_name - r.name.len());
        println!("{} {}{} {}", r.status.icon(), r.name, padding, r.message);
        if let Some(ref rem) = r.remediation {
            println!("  {} → {}", " ".repeat(max_name), rem);
        }
    }
    println!();
    println!(
        "doctor: {} warning(s), {} failure(s)",
        summary.warn, summary.fail
    );
}

// ── Check 1 — binary-integrity ───────────────────────────────────────────────

/// Compare the compile-time version against what `--version` reports.
///
/// Note: the executable-bit check from the bead description is implicitly
/// satisfied if we reached this point — the OS ran us, so the bit is set.
fn check_binary_integrity() -> CheckResult {
    const NAME: &str = "binary-integrity";
    let compile_ver = env!("CARGO_PKG_VERSION");

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("could not determine current executable: {e}"),
                "re-install cofferdam",
            );
        }
    };

    let output = match std::process::Command::new(&exe).arg("--version").output() {
        Ok(o) => o,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("failed to exec {:?} --version: {e}", exe),
                "re-install cofferdam",
            );
        }
    };

    if !output.status.success() {
        return CheckResult::fail(
            NAME,
            format!(
                "`cofferdam --version` exited with code {:?}",
                output.status.code()
            ),
            "re-install cofferdam",
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // clap's `--version` output is like "cofferdam 0.1.4\n"
    if stdout.contains(compile_ver) {
        CheckResult::pass(NAME, format!("{compile_ver} (matches build)"))
    } else {
        CheckResult::fail(
            NAME,
            format!(
                "compile-time version {compile_ver} differs from --version output: {}",
                stdout.trim()
            ),
            "re-install cofferdam to get a consistent binary",
        )
    }
}

// ── Check 2 — config ─────────────────────────────────────────────────────────

fn check_config() -> CheckResult {
    const NAME: &str = "config";

    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("could not determine current directory: {e}"),
                "check your working directory",
            );
        }
    };

    let path = match cfg::discover(&cwd) {
        Some(p) => p,
        None => {
            return CheckResult::pass(NAME, "no cofferdam.toml found (defaults in effect)");
        }
    };

    // Attempt to load and parse.
    match cfg::load(&path) {
        Ok(project_cfg) => {
            let registered: Vec<&str> = all_builtins().iter().map(|c| c.meta().id).collect();
            let unknown = cfg::unknown_check_ids(&project_cfg, &registered);
            if unknown.is_empty() {
                CheckResult::pass(NAME, format!("cofferdam.toml at {}", path.display()))
            } else {
                let ids: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
                CheckResult::warn(
                    NAME,
                    format!(
                        "{} — unknown check id(s): {}",
                        path.display(),
                        ids.join(", ")
                    ),
                    "remove the unknown key(s) or upgrade cofferdam",
                )
            }
        }
        Err(e) => CheckResult::fail(
            NAME,
            format!("failed to parse {}: {e}", path.display()),
            format!("fix the syntax error in {}", path.display()),
        ),
    }
}

// ── Check 3 — invariants-layers ──────────────────────────────────────────────

/// Warn when `cofferdam.invariants.toml` is present but has an empty or absent
/// `[layers]` block. In that state `Design.LayerViolation` silently checks
/// nothing — a common footgun. If the file is missing entirely we pass
/// (the user hasn't opted into layer enforcement, which is fine).
fn check_invariants_layers() -> CheckResult {
    const NAME: &str = "invariants-layers";
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("could not determine current directory: {e}"),
                "check your working directory",
            );
        }
    };
    check_invariants_layers_at(&cwd)
}

/// Test-friendly variant: search for `cofferdam.invariants.toml` starting
/// from `start` rather than process-global CWD. Keeps doctor tests free of
/// `std::env::set_current_dir` mutations, which race when test threads run
/// in parallel.
fn check_invariants_layers_at(start: &Path) -> CheckResult {
    const NAME: &str = "invariants-layers";

    // Reuse the same walk-up discovery the engine uses.
    let path = match invariants::discover(start) {
        Some(p) => p,
        None => {
            return CheckResult::pass(
                NAME,
                "no cofferdam.invariants.toml — Design.LayerViolation skipped",
            );
        }
    };

    // Parse the file.  A broken invariants.toml is surfaced as a fail
    // here (it means the file exists but is unusable), matching the tone
    // of check_config().
    let spec = match invariants::load(&path) {
        Ok(s) => s,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("failed to parse {}: {e}", path.display()),
                format!("fix the syntax error in {}", path.display()),
            );
        }
    };

    // TODO(cd-5lh): same warn for [invariants] / [public_api]
    if spec.layers.is_empty() {
        CheckResult::warn(
            NAME,
            format!(
                "{} present but [layers] is empty; Design.LayerViolation will not fire",
                path.display()
            ),
            "add a [layers] block — see https://tajd.github.io/cofferdam/invariants/",
        )
    } else {
        let count = spec.layers.len();
        CheckResult::pass(
            NAME,
            format!("{} — {} layer(s) declared", path.display(), count),
        )
    }
}

// ── Check 4 — baseline ───────────────────────────────────────────────────────

fn check_baseline() -> CheckResult {
    const NAME: &str = "baseline";

    // Resolve the baseline path using the same walk-up the CLI uses.
    let baseline_path = match resolve_baseline_path() {
        Some(p) => p,
        None => {
            return CheckResult::pass(NAME, "no baseline file (running unbaselined)");
        }
    };

    match baseline::read(&baseline_path) {
        Ok(bl) => {
            let count = bl.findings.len();
            // Check for entries that reference files no longer in the repo.
            let missing = stale_baseline_files(&bl, &baseline_path);
            if missing.is_empty() {
                CheckResult::pass(
                    NAME,
                    format!(
                        "{} — {} baselined finding(s)",
                        baseline_path.display(),
                        count
                    ),
                )
            } else {
                let truncated = format_truncated_list(&missing, 5);
                CheckResult::warn(
                    NAME,
                    format!(
                        "{} baselined file(s) no longer exist: {}",
                        missing.len(),
                        truncated
                    ),
                    "cofferdam baseline write",
                )
            }
        }
        Err(baseline::BaselineError::Version {
            found, supported, ..
        }) => CheckResult::warn(
            NAME,
            format!("baseline schema is v{found}; this binary writes v{supported} — regenerate it"),
            "cofferdam baseline write",
        ),
        Err(e) => CheckResult::fail(
            NAME,
            format!("failed to load {}: {e}", baseline_path.display()),
            "cofferdam baseline write",
        ),
    }
}

fn resolve_baseline_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();
    loop {
        let candidate = dir.join(BASELINE_DEFAULT_PATH);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// Return a list of distinct file paths referenced by the baseline that
/// no longer exist on disk.  File paths in the baseline are repo-relative;
/// we resolve them against the project root (parent of `.cofferdam/`).
fn stale_baseline_files(bl: &Baseline, baseline_path: &Path) -> Vec<String> {
    let project_root = baseline_path
        .parent() // .cofferdam/
        .and_then(|p| p.parent()); // project root

    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut missing: Vec<String> = Vec::new();

    for entry in &bl.findings {
        if seen.contains(entry.file.as_str()) {
            continue;
        }
        seen.insert(&entry.file);
        let exists = match project_root {
            Some(root) => root.join(&*entry.file).exists(),
            None => Path::new(&entry.file).exists(),
        };
        if !exists {
            missing.push(entry.file.clone());
        }
    }
    missing.sort();
    missing
}

// ── Check 5 — baseline-suppression ───────────────────────────────────────────

fn check_baseline_suppression() -> CheckResult {
    const NAME: &str = "baseline-suppression";
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("could not determine current directory: {e}"),
                "check your working directory",
            );
        }
    };
    check_baseline_suppression_at(&cwd)
}

/// Test-friendly variant: takes the directory to search from.
pub fn check_baseline_suppression_at(start: &Path) -> CheckResult {
    const NAME: &str = "baseline-suppression";

    match baseline_lint::count_dual_state_at(start) {
        Ok(0) => CheckResult::pass(
            NAME,
            "no dual-state entries (baseline ∩ inline suppressions = ∅)",
        ),
        Ok(n) => CheckResult::warn(
            NAME,
            format!("{n} baseline entr(ies) also have inline suppressions"),
            "run `cofferdam baseline lint` for the full list",
        ),
        Err(e) => CheckResult::warn(
            NAME,
            format!("could not check baseline suppression overlap: {e}"),
            "ensure the baseline file is readable",
        ),
    }
}

// ── Check 6 (was 5) — git ─────────────────────────────────────────────────────

fn check_git() -> CheckResult {
    const NAME: &str = "git";

    // Check that `git --version` exits 0.
    let ver_output = match std::process::Command::new("git").arg("--version").output() {
        Ok(o) => o,
        Err(_) => {
            return CheckResult::fail(
                NAME,
                "git not found on PATH",
                "install git for `--since` and CI integration",
            );
        }
    };

    if !ver_output.status.success() {
        return CheckResult::fail(
            NAME,
            "git exited non-zero on --version",
            "install git for `--since` and CI integration",
        );
    }

    let git_ver = String::from_utf8_lossy(&ver_output.stdout)
        .trim()
        .to_string();

    // Check that cwd is inside a git repo.
    let repo_output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();

    match repo_output {
        Ok(o) if o.status.success() => CheckResult::pass(NAME, format!("{git_ver} — in repo")),
        _ => CheckResult::warn(
            NAME,
            format!("{git_ver} — not inside a git checkout"),
            "run cofferdam from inside a git checkout if you need `--since`",
        ),
    }
}

// ── Check 6 — discovery ──────────────────────────────────────────────────────

fn check_discovery() -> CheckResult {
    const NAME: &str = "discovery";

    let opts = DiscoveryOptions::default();
    let roots = vec![PathBuf::from(".")];

    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            return CheckResult::fail(
                NAME,
                format!("discovery error: {e}"),
                "check directory permissions and .cofferdamignore",
            );
        }
    };

    if files.is_empty() {
        // Check whether there is a cofferdam.toml with a custom discovery scope;
        // if so, the user intentionally narrowed scope.
        let has_explicit_scope = has_discovery_scope_in_config();
        if has_explicit_scope {
            CheckResult::pass(
                NAME,
                "0 TypeScript files (explicit scope in cofferdam.toml)",
            )
        } else {
            CheckResult::warn(
                NAME,
                "no TypeScript files found in default scope",
                "scope cofferdam by passing paths or configuring `[discovery]` in cofferdam.toml",
            )
        }
    } else {
        CheckResult::pass(NAME, format!("{} TypeScript file(s)", files.len()))
    }
}

/// Returns true if a cofferdam.toml is present AND contains a `[discovery]`
/// table (a heuristic for intentional scope narrowing).
fn has_discovery_scope_in_config() -> bool {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let path = match cfg::discover(&cwd) {
        Some(p) => p,
        None => return false,
    };
    // Just read the raw TOML; we don't need to fully parse it.
    match std::fs::read_to_string(&path) {
        Ok(raw) => raw.contains("[discovery]"),
        Err(_) => false,
    }
}

// ── Check 7 — suppression-directives ─────────────────────────────────────────

fn check_suppression_directives() -> CheckResult {
    const NAME: &str = "suppression-directives";
    const FILE_LIMIT: usize = 1000;

    let opts = DiscoveryOptions::default();
    let roots = vec![PathBuf::from(".")];

    let files = match discover(&roots, &opts) {
        Ok(f) => f,
        Err(e) => {
            return CheckResult::warn(
                NAME,
                format!("discovery error during suppression scan: {e}"),
                "check directory permissions",
            );
        }
    };

    if files.len() > FILE_LIMIT {
        return CheckResult::warn(
            NAME,
            format!(
                "skipped — {} files exceeds scan limit of {}; pass --paths to scope",
                files.len(),
                FILE_LIMIT
            ),
            "run `cofferdam doctor` from a narrower directory or add --paths",
        );
    }

    let known_ids: std::collections::HashSet<&str> =
        all_builtins().iter().map(|c| c.meta().id).collect();

    let mut bad_refs: Vec<String> = Vec::new();

    for file in &files {
        let text = match std::fs::read_to_string(file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Use the suppress module to extract mentioned check IDs.
        let mentioned = suppress::mentioned_check_ids(&text);
        for (check_id, line) in mentioned {
            if !known_ids.contains(check_id.as_str()) {
                bad_refs.push(format!("{}:{} {}", file.display(), line, check_id));
            }
        }
    }

    if bad_refs.is_empty() {
        CheckResult::pass(NAME, "all directive IDs valid")
    } else {
        let count = bad_refs.len();
        let truncated = format_truncated_list(&bad_refs, 5);
        CheckResult::warn(
            NAME,
            format!("{count} directive(s) reference unknown check ID(s): {truncated}"),
            "rename to a current check ID or remove the directive — see `cofferdam explain --list`",
        )
    }
}

// ── Check 8 — type-host ──────────────────────────────────────────────────────

/// Warn when type-aware checks are registered but no root `tsconfig.json`
/// can be found — the common pnpm/turbo monorepo footgun (a
/// `tsconfig.base.json` plus per-package configs, but nothing at the
/// project root). Mirrors the discovery predicate in `main.rs`'s
/// `find_tsconfig`, duplicated locally so this check stays independent of
/// the CLI's type-host wiring.
fn check_type_host() -> CheckResult {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return CheckResult::fail(
                "type-host",
                format!("could not determine current directory: {e}"),
                "check your working directory",
            );
        }
    };
    check_type_host_at(&cwd)
}

/// Test-friendly variant: takes the directory to search from.
fn check_type_host_at(start: &Path) -> CheckResult {
    const NAME: &str = "type-host";

    let needs_types = all_builtins().iter().any(|c| c.meta().requires_types);
    if !needs_types {
        return CheckResult::pass(NAME, "no type-aware checks registered");
    }

    if find_tsconfig_from(start).is_some() {
        CheckResult::pass(NAME, "root tsconfig.json found — type-aware checks enabled")
    } else {
        CheckResult::warn(
            NAME,
            format!(
                "type-aware checks are registered but no tsconfig.json found near {} — they will be skipped",
                start.display()
            ),
            "add a root tsconfig.json (a references-only one is enough) so type-aware checks can run",
        )
    }
}

/// Walk up from `start` looking for a `tsconfig.json`. Mirrors
/// `main.rs::find_tsconfig`.
fn find_tsconfig_from(start: &Path) -> Option<PathBuf> {
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

// ── Check 9 — wrapper-version ────────────────────────────────────────────────

fn check_wrapper_version() -> CheckResult {
    const NAME: &str = "wrapper-version";
    let compile_ver = env!("CARGO_PKG_VERSION");

    match find_npm_package_json() {
        None => CheckResult::skipped(NAME, "not an npm install — skipping wrapper-version check"),
        Some(pkg_path) => match read_npm_version(&pkg_path) {
            Ok(npm_ver) => {
                if npm_ver == compile_ver {
                    CheckResult::pass(
                        NAME,
                        format!("{npm_ver} (npm package and binary version match)"),
                    )
                } else {
                    CheckResult::warn(
                            NAME,
                            format!(
                                "npm package version {npm_ver} differs from binary version {compile_ver}"
                            ),
                            "this is an intentional pre-1.0 drift if you're working in the source \
                             tree; for npm-installed users: re-run `npm install cofferdam`",
                        )
                }
            }
            Err(e) => CheckResult::warn(
                NAME,
                format!("could not read {}: {e}", pkg_path.display()),
                "verify the npm package.json is intact",
            ),
        },
    }
}

/// Walk up from the current executable looking for
/// `packages/cofferdam/package.json` alongside a repo root marker.
fn find_npm_package_json() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;

    // Walk up at most 10 levels looking for the repo root marker alongside
    // a `packages/cofferdam/package.json` sibling.
    for _ in 0..10 {
        let candidate = dir.join("packages/cofferdam/package.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
    None
}

fn read_npm_version(pkg_path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(pkg_path).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    v["version"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "missing or non-string `version` field".into())
}

// ── Check 10 — formatter-coexistence ─────────────────────────────────────────

/// Report how cofferdam coexists with a formatter/linter. CD-357 removed
/// every built-in that duplicated a rule biome and eslint ship out of the
/// box, so there is no longer anything to double-report and this check has
/// no failure mode — it exists to say so, since users arriving from a
/// linter reasonably expect an overlap.
fn check_formatter_coexistence() -> CheckResult {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return CheckResult::fail(
                "formatter-coexistence",
                format!("could not determine current directory: {e}"),
                "check your working directory",
            );
        }
    };
    check_formatter_coexistence_at(&cwd)
}

/// Test-friendly variant: takes the directory to search from (no parent
/// walk-up — a `biome.json`/eslint config always lives at the project
/// root, same place `cofferdam.toml` does).
fn check_formatter_coexistence_at(start: &Path) -> CheckResult {
    const NAME: &str = "formatter-coexistence";

    let biome = ["biome.json", "biome.jsonc"]
        .iter()
        .find(|f| start.join(f).is_file());
    let eslint = [
        "eslint.config.js",
        "eslint.config.mjs",
        "eslint.config.cjs",
        "eslint.config.ts",
        ".eslintrc",
        ".eslintrc.json",
        ".eslintrc.js",
        ".eslintrc.cjs",
        ".eslintrc.yml",
        ".eslintrc.yaml",
    ]
    .iter()
    .find(|f| start.join(f).is_file());

    let found = match (biome, eslint) {
        (Some(f), _) => Some(("biome", *f)),
        (None, Some(f)) => Some(("eslint", *f)),
        (None, None) => None,
    };

    match found {
        None => CheckResult::pass(
            NAME,
            "no biome/eslint config found — nothing to coexist with",
        ),
        Some((tool, file)) => CheckResult::pass(
            NAME,
            format!("{file} found — no built-in check overlaps {tool}, so both can run unmodified"),
        ),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Format up to `limit` items from `list`, appending `... and N more` when
/// the list is longer.
fn format_truncated_list(list: &[String], limit: usize) -> String {
    if list.len() <= limit {
        list.join(", ")
    } else {
        let shown: Vec<&str> = list[..limit].iter().map(|s| s.as_str()).collect();
        let remaining = list.len() - limit;
        format!("{}, ... and {} more", shown.join(", "), remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_truncated_short_list_no_ellipsis() {
        let items: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(format_truncated_list(&items, 5), "a, b, c");
    }

    #[test]
    fn format_truncated_long_list_adds_more() {
        let items: Vec<String> = (0..8).map(|i| i.to_string()).collect();
        let out = format_truncated_list(&items, 5);
        assert!(out.contains("... and 3 more"), "got: {out}");
    }

    #[test]
    fn check_result_pass_has_no_remediation() {
        let r = CheckResult::pass("test", "all good");
        assert_eq!(r.status, Status::Pass);
        assert!(r.remediation.is_none());
    }

    #[test]
    fn check_result_fail_has_remediation() {
        let r = CheckResult::fail("test", "broken", "fix it");
        assert_eq!(r.status, Status::Fail);
        assert!(r.remediation.is_some());
    }

    #[test]
    fn status_icons_match_expected() {
        assert_eq!(Status::Pass.icon(), "✓");
        assert_eq!(Status::Warn.icon(), "⚠");
        assert_eq!(Status::Fail.icon(), "✗");
        assert_eq!(Status::Skipped.icon(), "-");
    }

    // ── check_invariants_layers tests ─────────────────────────────────────────

    /// Helper: run `check_invariants_layers_at` rooted at a tempdir.
    /// Avoids `std::env::set_current_dir` so parallel test threads cannot
    /// race on process-global CWD.
    fn run_invariants_check_in(dir: &std::path::Path) -> CheckResult {
        check_invariants_layers_at(dir)
    }

    #[test]
    fn check_passes_when_invariants_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = run_invariants_check_in(dir.path());
        assert_eq!(
            r.status,
            Status::Pass,
            "missing file should be pass: {}",
            r.message
        );
        assert_eq!(r.name, "invariants-layers");
    }

    #[test]
    fn check_warns_when_invariants_layers_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        // [layers] section present but no entries beneath it.
        std::fs::write(dir.path().join("cofferdam.invariants.toml"), "[layers]\n").expect("write");
        let r = run_invariants_check_in(dir.path());
        assert_eq!(
            r.status,
            Status::Warn,
            "empty [layers] should warn: {}",
            r.message
        );
        assert!(
            r.message.contains("empty") || r.message.contains("will not fire"),
            "message should mention empty/will not fire: {}",
            r.message
        );
        assert!(r.remediation.is_some());
    }

    #[test]
    fn check_warns_when_invariants_layers_section_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        // File present but has no [layers] at all — only another block.
        std::fs::write(
            dir.path().join("cofferdam.invariants.toml"),
            "[invariants]\n",
        )
        .expect("write");
        let r = run_invariants_check_in(dir.path());
        assert_eq!(
            r.status,
            Status::Warn,
            "[layers] absent should warn: {}",
            r.message
        );
        assert!(
            r.message.contains("empty") || r.message.contains("will not fire"),
            "message should mention empty/will not fire: {}",
            r.message
        );
    }

    // ── check_baseline_suppression tests ──────────────────────────────────────

    fn write_baseline(
        dir: &std::path::Path,
        entries: Vec<cofferdam_engine::baseline::BaselineEntry>,
    ) {
        let bl = cofferdam_engine::baseline::Baseline::new(entries);
        let target = dir.join(".cofferdam/baseline.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        cofferdam_engine::baseline::write(&target, &bl).unwrap();
    }

    #[test]
    fn baseline_suppression_passes_when_no_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = check_baseline_suppression_at(dir.path());
        assert_eq!(r.status, Status::Pass, "no baseline → pass: {}", r.message);
        assert_eq!(r.name, "baseline-suppression");
    }

    #[test]
    fn baseline_suppression_passes_when_no_overlap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("a.ts"), "console.log(\"hi\");\n").unwrap();
        write_baseline(
            dir.path(),
            vec![cofferdam_engine::baseline::BaselineEntry {
                file: "src/a.ts".into(),
                check_id: "Warning.NoConsoleLog".into(),
                rule_signature: "sig".into(),
            }],
        );
        let r = check_baseline_suppression_at(dir.path());
        assert_eq!(r.status, Status::Pass, "no overlap → pass: {}", r.message);
    }

    #[test]
    fn baseline_suppression_warns_when_overlap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("a.ts"),
            "// cofferdam-ignore: Warning.NoConsoleLog: legacy\nconsole.log(\"hi\");\n",
        )
        .unwrap();
        write_baseline(
            dir.path(),
            vec![cofferdam_engine::baseline::BaselineEntry {
                file: "src/a.ts".into(),
                check_id: "Warning.NoConsoleLog".into(),
                rule_signature: "sig".into(),
            }],
        );
        let r = check_baseline_suppression_at(dir.path());
        assert_eq!(r.status, Status::Warn, "overlap → warn: {}", r.message);
        assert!(r.remediation.is_some());
        assert!(
            r.message.contains("dual-state") || r.message.contains("also have"),
            "message should mention dual-state: {}",
            r.message
        );
    }

    // ── check_type_host tests ─────────────────────────────────────────────────

    #[test]
    fn type_host_passes_when_tsconfig_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tsconfig.json"), "{}\n").expect("write");
        let r = check_type_host_at(dir.path());
        assert_eq!(r.name, "type-host");
        // Whether this is a Pass depends on whether any built-in check
        // currently requires types; either way tsconfig presence must not
        // produce the "no tsconfig" warning message.
        assert!(
            !r.message.contains("no tsconfig.json found"),
            "should not warn when tsconfig.json exists: {}",
            r.message
        );
    }

    #[test]
    fn type_host_warns_when_tsconfig_missing_and_types_required() {
        let dir = tempfile::tempdir().expect("tempdir");
        let needs_types = all_builtins().iter().any(|c| c.meta().requires_types);
        let r = check_type_host_at(dir.path());
        if needs_types {
            assert_eq!(
                r.status,
                Status::Warn,
                "missing tsconfig with type-aware checks registered should warn: {}",
                r.message
            );
            assert!(r.remediation.is_some());
        } else {
            assert_eq!(r.status, Status::Pass);
        }
    }

    #[test]
    fn find_tsconfig_from_walks_up_parent_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tsconfig.json"), "{}\n").expect("write");
        let nested = dir.path().join("packages/app");
        std::fs::create_dir_all(&nested).expect("mkdir");
        assert!(find_tsconfig_from(&nested).is_some());
    }

    #[test]
    fn find_tsconfig_from_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(find_tsconfig_from(dir.path()).is_none());
    }

    // ── check_formatter_coexistence tests ─────────────────────────────────────

    #[test]
    fn formatter_coexistence_passes_when_no_config_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let r = check_formatter_coexistence_at(dir.path());
        assert_eq!(r.status, Status::Pass, "no config → pass: {}", r.message);
        assert_eq!(r.name, "formatter-coexistence");
    }

    #[test]
    fn formatter_coexistence_passes_on_biome_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("biome.json"), "{}\n").expect("write");
        let r = check_formatter_coexistence_at(dir.path());
        assert_eq!(r.status, Status::Pass, "biome.json → pass: {}", r.message);
        assert!(r.message.contains("biome.json"), "got: {}", r.message);
        assert!(r.remediation.is_none(), "nothing to remediate");
    }

    #[test]
    fn formatter_coexistence_passes_on_eslint_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("eslint.config.js"), "").expect("write");
        let r = check_formatter_coexistence_at(dir.path());
        assert_eq!(
            r.status,
            Status::Pass,
            "eslint config → pass: {}",
            r.message
        );
        assert!(r.message.contains("eslint"), "got: {}", r.message);
    }

    #[test]
    fn formatter_coexistence_biome_takes_precedence_over_eslint() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("biome.json"), "{}\n").expect("write");
        std::fs::write(dir.path().join(".eslintrc.json"), "{}\n").expect("write");
        let r = check_formatter_coexistence_at(dir.path());
        assert!(r.message.contains("biome"), "got: {}", r.message);
    }

    #[test]
    fn check_passes_when_invariants_layers_populated() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("cofferdam.invariants.toml"),
            "[layers]\nui = [\"src/ui/**\"]\n",
        )
        .expect("write");
        let r = run_invariants_check_in(dir.path());
        assert_eq!(
            r.status,
            Status::Pass,
            "populated [layers] should pass: {}",
            r.message
        );
        assert!(
            r.message.contains("layer(s)"),
            "message should mention layer count: {}",
            r.message
        );
    }
}
