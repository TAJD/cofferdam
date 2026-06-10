//! cd-9hp.4 cp2 — findings-cache no-op re-run regression budget.
//!
//! Measures the wall-clock speedup of a no-op re-run of
//! [`cofferdam_engine::Engine::analyze_with_sources_caches`] (with
//! both parse + findings caches) against the equivalent cold run.
//!
//! ## What changes vs cp1
//!
//! The cp1 parse cache shaved only the oxc-parse step (~10% of
//! release-mode cost on bestefforttools, ~1.1× speedup). The cp2
//! findings cache adds skipping `Check::run` for every check that
//! opts into [`cofferdam_core::CheckMeta::pure_run`] — the majority
//! of TS checks. The bead projected ≥7×; the realistic number
//! depends on (a) how many builtin checks are pure vs. corpus-
//! writers, (b) how much of the engine's time is per-check work
//! vs. graph extract + finalize. Per-file findings cache cannot
//! shave graph extract or finalize — those land in cp3.
//!
//! Asserts a generous floor (`HARD_FLOOR`) so transient CI noise
//! doesn't flake the test; the measured number is recorded in
//! `DOCUMENTED_BUDGET` so a later checkpoint that touches the
//! cache without retuning surfaces the change in the printed line.
//!
//! ## Timing-assertion convention (cd-mhks)
//!
//! The wall-clock ratio assertion is tagged `#[ignore]` so it is
//! excluded from the default test run (and the pre-push hook, which
//! runs `cargo clippy -D warnings` concurrently with `cargo test`).
//! Functional assertions (hit/miss counters, findings parity) remain
//! in `findings_cache_correctness` and run by default.
//! Run the timing gate explicitly:
//!
//!   cargo test -p cofferdam-engine -- --ignored
//!
//! ## Target repo
//!
//! Configurable via `COFFERDAM_BENCH_REPO`. Falls back to
//! `C:/Users/tajdi/bestefforttools`. Skipped without a target.

use std::path::{Path, PathBuf};
use std::time::Instant;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{
    cache::ParseCache, discover, findings_cache::FindingsCache, DiscoveryOptions, Engine,
};
use serde::Serialize;

/// Measured speedup ratio on bestefforttools, release build,
/// dev box. Tracks the COMBINED parse + findings cache
/// contribution to no-op re-run cost. The bead's cp2 projection
/// was ≥7×; reality on this fixture is ~1.3× because
/// `graph_builder.collect` (engine-level, every pass) plus the
/// non-pure cross-file checks (DRY, UnusedImport, OrphanExport
/// finalize, etc.) dominate. cp3's corpus snapshot replay is
/// where the further speedup lives — skip graph extract + non-pure
/// check runs on no-op re-runs.
const DOCUMENTED_BUDGET: f64 = 1.3;
/// Below this and we flake the test rather than silently ship a
/// regression. cp2 contributes a strictly positive (though small)
/// speedup over cp1; anything at or below 1.0× means the warm pass
/// paid more than cold — broken cache or broken integration.
const HARD_FLOOR: f64 = 1.05;

#[derive(Debug, Serialize)]
struct BenchResult {
    repo: String,
    ts_files: usize,
    cold_ms: f64,
    warm_ms: f64,
    speedup_ratio: f64,
    parse_cache_hits: u64,
    parse_cache_misses: u64,
    findings_cache_hits: u64,
    findings_cache_misses: u64,
    findings_cache_entries: usize,
    documented_budget: f64,
    hard_floor: f64,
}

fn target_repo() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("COFFERDAM_BENCH_REPO") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let default = PathBuf::from("C:/Users/tajdi/bestefforttools");
    default.exists().then_some(default)
}

fn results_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root two levels up from crate manifest");
    workspace_root
        .join("tests")
        .join("findings-cache-bench-results")
}

/// Run the findings-cache bench. Returns `None` when no target repo is available.
///
/// Runs all functional assertions inline (hit/miss counters, findings parity).
/// Timing data is returned so the `#[ignore]` timing test can assert against it.
fn run_findings_cache_bench() -> Option<BenchResult> {
    let Some(repo) = target_repo() else {
        println!(
            "[findings_cache_bench] no bench repo found; set COFFERDAM_BENCH_REPO \
             or place a TS repo at C:/Users/tajdi/bestefforttools. Skipping."
        );
        return None;
    };

    let discovery_opts = DiscoveryOptions::default();
    let files = discover(std::slice::from_ref(&repo), &discovery_opts).expect("discover");
    println!(
        "[findings_cache_bench] {} TS files in {}",
        files.len(),
        repo.display()
    );

    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|t| (p.clone(), t)))
        .collect();
    assert!(
        !sources.is_empty(),
        "[findings_cache_bench] target repo has no readable TS files"
    );

    let engine = Engine::new(all_builtins());
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();

    // Cold pass: both caches empty.
    let t_cold = Instant::now();
    let (cold_issues, _) = engine.analyze_with_sources_caches(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
    );
    let cold_ns = t_cold.elapsed().as_nanos();
    let parse_misses_after_cold = parse_cache.misses();
    let findings_misses_after_cold = findings_cache.misses();

    // Warm pass: same sources, populated caches. Every pure-check
    // lookup should hit; non-pure checks still run (the bead's
    // cp3 territory).
    let t_warm = Instant::now();
    let (warm_issues, _) =
        engine.analyze_with_sources_caches(sources, Some(&parse_cache), Some(&findings_cache));
    let warm_ns = t_warm.elapsed().as_nanos();

    assert_eq!(
        cold_issues.len(),
        warm_issues.len(),
        "warm pass must produce the same findings count as cold"
    );
    assert_eq!(
        parse_cache.misses(),
        parse_misses_after_cold,
        "warm pass added new parse misses ({} → {}) — parse cache lost a key",
        parse_misses_after_cold,
        parse_cache.misses()
    );
    assert_eq!(
        findings_cache.misses(),
        findings_misses_after_cold,
        "warm pass added new findings misses ({} → {}) — findings cache lost a key",
        findings_misses_after_cold,
        findings_cache.misses()
    );

    let cold_ms = cold_ns as f64 / 1e6;
    let warm_ms = warm_ns as f64 / 1e6;
    let speedup_ratio = if warm_ms > 0.0 {
        cold_ms / warm_ms
    } else {
        f64::INFINITY
    };

    let result = BenchResult {
        repo: repo.display().to_string(),
        ts_files: parse_misses_after_cold as usize,
        cold_ms,
        warm_ms,
        speedup_ratio,
        parse_cache_hits: parse_cache.hits(),
        parse_cache_misses: parse_cache.misses(),
        findings_cache_hits: findings_cache.hits(),
        findings_cache_misses: findings_cache.misses(),
        findings_cache_entries: findings_cache.len(),
        documented_budget: DOCUMENTED_BUDGET,
        hard_floor: HARD_FLOOR,
    };

    println!();
    println!("[findings_cache_bench] results");
    println!("  repo:                {}", result.repo);
    println!("  TS files cached:     {}", result.ts_files);
    println!("  cold:                {:>10.2} ms", cold_ms);
    println!("  warm:                {:>10.2} ms", warm_ms);
    println!(
        "  speedup:             {:>10.2}× (budget {:.1}×, floor {:.1}×)",
        speedup_ratio, DOCUMENTED_BUDGET, HARD_FLOOR
    );
    println!(
        "  parse  hits/misses:  {} / {}",
        result.parse_cache_hits, result.parse_cache_misses
    );
    println!(
        "  findings hits/misses: {} / {}",
        result.findings_cache_hits, result.findings_cache_misses
    );
    println!("  findings entries:    {}", result.findings_cache_entries);
    println!();

    let dir = results_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let out = dir.join(format!("run-{ts}.json"));
        let _ = std::fs::write(
            &out,
            serde_json::to_string_pretty(&result).expect("serialize"),
        );
    }

    Some(result)
}

/// Functional correctness: parse + findings cache hit/miss counters and findings parity.
///
/// Wall-clock timing gate lives in [`findings_cache_no_op_rerun_within_budget`]
/// (`#[ignore]`), excluded from the default run to avoid pre-push hook flakes.
#[test]
fn findings_cache_correctness() {
    run_findings_cache_bench();
}

// cd-mhks: timing assertion excluded from the default run — flakes when the
// pre-push hook runs cargo clippy concurrently with cargo test (CPU contention
// causes the warm pass to occasionally miss the speedup floor).
// Run explicitly: cargo test -p cofferdam-engine -- --ignored
#[test]
#[ignore]
fn findings_cache_no_op_rerun_within_budget() {
    let Some(result) = run_findings_cache_bench() else {
        return;
    };
    assert!(
        result.speedup_ratio >= HARD_FLOOR,
        "[findings_cache_bench] regression: speedup {:.2}× < {:.1}× floor (documented budget {:.1}×); \
         cold={:.2}ms warm={:.2}ms",
        result.speedup_ratio, HARD_FLOOR, DOCUMENTED_BUDGET, result.cold_ms, result.warm_ms
    );
}
