//! Corpus smoke-test / benchmark.
//!
//! Walks each directory under `tests/corpus/`, runs `Engine::analyze` with
//! all built-in checks, collects metrics (file count, finding count, parse
//! errors, wall time), and writes a JSON results file to
//! `tests/corpus-results/<timestamp>.json`.
//!
//! # When the corpus is absent
//! The test exits OK immediately with a notice printed to stdout. CI without
//! the corpus must not fail. To populate the corpus run:
//!
//!   bash scripts/fetch-corpus.sh
//!
//! # Perf assertion
//! Each repo must complete within 30 s (30_000 ms). The bound is intentionally
//! generous — it must never flake on a slow CI runner — while still catching a
//! catastrophic regression (e.g. an O(n²) scan over a large repo). The actual
//! wall times on a developer laptop with a warm filesystem cache are expected
//! to be under 5 s per repo. If a repo genuinely approaches 30 s, raise the
//! bound here and document the actual time.
//!
//! The perf assertion lives in `corpus_smoke_timing` (`#[ignore]`) so it is
//! excluded from the default test run and the pre-push hook (cd-mhks). Run it
//! explicitly: `cargo test -p cofferdam-engine -- --ignored`
//!
//! # Panics
//! Any panic during per-repo analysis is caught via `catch_unwind`, reported
//! to stdout, and re-panicked at the end so the test fails. We never swallow
//! a panic silently.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{discover, DiscoveryOptions, Engine};
use serde::Serialize;

// Per-repo wall-clock limit used by the `#[ignore]` timing test.
const WALL_MS_LIMIT: u128 = 30_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RepoResult {
    name: String,
    ts_files: usize,
    findings: usize,
    parse_errors: usize,
    wall_ms: u128,
    status: String,
}

#[derive(Debug, Serialize)]
struct RunResults {
    timestamp_unix_ms: u128,
    repos: Vec<RepoResult>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the path to `tests/corpus` relative to the workspace root.
///
/// The test binary's working directory at runtime is the workspace root when
/// run via `cargo test`, so we use `env!("CARGO_MANIFEST_DIR")` (which is
/// the `cofferdam-engine` crate root) and walk up two directories to reach the
/// workspace root, then append `tests/corpus`.
fn corpus_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/cofferdam-engine` at compile time.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    // workspace root = manifest/../..
    let workspace_root = manifest
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("workspace root should be two levels up from crate manifest");
    workspace_root.join("tests").join("corpus")
}

fn results_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    workspace_root.join("tests").join("corpus-results")
}

/// Collect subdirectories of `corpus_dir` — each is one repo.
fn corpus_repos(corpus: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(corpus) else {
        return vec![];
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
}

/// Run the corpus bench. Returns `None` when the corpus directory is absent.
///
/// Collects per-repo results (including wall_ms) without asserting on timing.
/// Timing assertions live in the `#[ignore]` test. The returned `panicked` list
/// is non-empty if any repo caused a panic — callers should re-panic on it.
fn run_corpus_bench() -> Option<(Vec<RepoResult>, Vec<String>)> {
    let corpus = corpus_dir();

    let repos = corpus_repos(&corpus);
    if repos.is_empty() {
        println!(
            "[corpus_smoke] corpus not present, skipping \
             (run scripts/fetch-corpus.sh)"
        );
        return None;
    }

    let engine = Engine::new(all_builtins());
    let discovery_opts = DiscoveryOptions::default();

    let mut results: Vec<RepoResult> = Vec::new();
    let mut panicked: Vec<String> = Vec::new();

    for repo_path in &repos {
        let repo_name = repo_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        // Catch any panic so one bad repo doesn't silently abort the others.
        // `Engine` contains `Box<dyn Check>` which is not `UnwindSafe`; we use
        // `AssertUnwindSafe` here because we are only reading engine state (not
        // mutating shared state in the panic path) and we re-panic at the end
        // anyway.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let files =
                discover(std::slice::from_ref(repo_path), &discovery_opts).unwrap_or_default();
            let ts_files = files.len();

            let t0 = Instant::now();
            let issues = engine
                .analyze(&files)
                .expect("engine.analyze should not error");
            let wall_ms = t0.elapsed().as_millis();

            let parse_errors = issues
                .iter()
                .filter(|i| i.check_id == "Warning.ParseError")
                .count();
            let findings = issues.len();

            (ts_files, findings, parse_errors, wall_ms)
        }));

        match outcome {
            Err(_) => {
                println!("[corpus_smoke] PANIC in repo: {repo_name}");
                panicked.push(repo_name.clone());
                results.push(RepoResult {
                    name: repo_name,
                    ts_files: 0,
                    findings: 0,
                    parse_errors: 0,
                    wall_ms: 0,
                    status: "PANIC".to_string(),
                });
            }
            Ok((ts_files, findings, parse_errors, wall_ms)) => {
                results.push(RepoResult {
                    name: repo_name,
                    ts_files,
                    findings,
                    parse_errors,
                    wall_ms,
                    status: "ok".to_string(),
                });
            }
        }
    }

    // --- Print results table ------------------------------------------------
    println!();
    println!(
        "{:<20} {:>10} {:>10} {:>13} {:>10}  status",
        "repo", "ts_files", "findings", "parse_errors", "wall_ms"
    );
    println!("{}", "-".repeat(80));
    for r in &results {
        println!(
            "{:<20} {:>10} {:>10} {:>13} {:>10}  {}",
            r.name, r.ts_files, r.findings, r.parse_errors, r.wall_ms, r.status
        );
    }
    println!();

    // --- Write JSON results file --------------------------------------------
    let results_dir = results_dir();
    if let Err(e) = std::fs::create_dir_all(&results_dir) {
        eprintln!("[corpus_smoke] could not create results dir: {e}");
    } else {
        let ts = now_unix_ms();
        let results_path = results_dir.join(format!("{ts}.json"));
        let run = RunResults {
            timestamp_unix_ms: ts,
            repos: results
                .iter()
                .map(|r| RepoResult {
                    name: r.name.clone(),
                    ts_files: r.ts_files,
                    findings: r.findings,
                    parse_errors: r.parse_errors,
                    wall_ms: r.wall_ms,
                    status: r.status.clone(),
                })
                .collect(),
        };
        match serde_json::to_string_pretty(&run) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&results_path, json) {
                    eprintln!("[corpus_smoke] could not write results: {e}");
                } else {
                    println!(
                        "[corpus_smoke] results written to {}",
                        results_path.display()
                    );
                }
            }
            Err(e) => eprintln!("[corpus_smoke] JSON serialisation failed: {e}"),
        }
    }

    Some((results, panicked))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Correctness smoke: engine must not panic on any corpus repo.
///
/// Wall-clock timing gate lives in [`corpus_smoke_timing`] (`#[ignore]`),
/// excluded from the default run to avoid pre-push hook flakes (cd-mhks).
#[test]
fn corpus_smoke() {
    let Some((_results, panicked)) = run_corpus_bench() else {
        return;
    };

    if !panicked.is_empty() {
        panic!(
            "[corpus_smoke] {} repo(s) panicked: {}",
            panicked.len(),
            panicked.join(", ")
        );
    }
}

// cd-mhks: timing assertion excluded from the default run — flakes under CPU
// load (pre-push hook runs cargo clippy concurrently with cargo test).
// Run explicitly: cargo test -p cofferdam-engine -- --ignored
#[test]
#[ignore]
fn corpus_smoke_timing() {
    let Some((results, panicked)) = run_corpus_bench() else {
        return;
    };

    if !panicked.is_empty() {
        panic!(
            "[corpus_smoke_timing] {} repo(s) panicked: {}",
            panicked.len(),
            panicked.join(", ")
        );
    }

    for r in &results {
        assert!(
            r.wall_ms < WALL_MS_LIMIT,
            "[corpus_smoke_timing] {}: analysis took {} ms, limit is {} ms",
            r.name,
            r.wall_ms,
            WALL_MS_LIMIT
        );
    }
}
