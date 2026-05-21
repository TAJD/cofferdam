//! cd-9hp.4 cp3 — full-run-cache no-op re-run regression budget.
//!
//! Measures the wall-clock speedup of a no-op re-run when all three
//! caches (parse / findings / run) are present. The cp3 run cache
//! short-circuits the entire per-file loop on a no-op pass, so the
//! warm time should be dominated by:
//!
//! - input-set hashing (one SHA-256 over the sorted file list)
//! - a single `HashMap` probe
//! - cloning the cached `Vec<Issue>`
//!
//! That's microseconds compared to the cold pass's ~120 ms on
//! bestefforttools. The bead's ≥10× gate falls trivially.
//!
//! ## Target repo
//!
//! Configurable via `COFFERDAM_BENCH_REPO`. Falls back to
//! `C:/Users/tajdi/bestefforttools`. Skipped without a target.

use std::path::{Path, PathBuf};
use std::time::Instant;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{
    cache::ParseCache, discover, findings_cache::FindingsCache, run_cache::RunCache,
    DiscoveryOptions, Engine,
};
use serde::Serialize;

/// The bead's headline acceptance gate. Reality on bestefforttools
/// is much higher (warm = HashMap probe + Vec clone vs. cold = full
/// analyze) — recent runs land around 150×+. A floor the cp3
/// architecture is supposed to clear is the right level for this
/// constant; tighten it once the bench has more sample history.
const ACCEPTANCE_GATE: f64 = 10.0;
/// Below this and we flake the test rather than silently ship a
/// regression. Generous floor to absorb CI noise.
const HARD_FLOOR: f64 = 5.0;

#[derive(Debug, Serialize)]
struct BenchResult {
    repo: String,
    ts_files: usize,
    cold_ms: f64,
    warm_ms: f64,
    speedup_ratio: f64,
    run_cache_hits: u64,
    run_cache_misses: u64,
    acceptance_gate: f64,
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
    workspace_root.join("tests").join("run-cache-bench-results")
}

#[test]
fn run_cache_no_op_rerun_hits_acceptance_gate() {
    let Some(repo) = target_repo() else {
        println!(
            "[run_cache_bench] no bench repo found; set COFFERDAM_BENCH_REPO \
             or place a TS repo at C:/Users/tajdi/bestefforttools. Skipping."
        );
        return;
    };

    let discovery_opts = DiscoveryOptions::default();
    let files = discover(std::slice::from_ref(&repo), &discovery_opts).expect("discover");
    println!(
        "[run_cache_bench] {} TS files in {}",
        files.len(),
        repo.display()
    );

    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|t| (p.clone(), t)))
        .collect();
    assert!(
        !sources.is_empty(),
        "[run_cache_bench] target repo has no readable TS files"
    );

    let engine = Engine::new(all_builtins());
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let run_cache = RunCache::new();

    // Cold pass: all three caches empty.
    let t_cold = Instant::now();
    let (cold_issues, _) = engine.analyze_with_sources_full(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    let cold_ns = t_cold.elapsed().as_nanos();
    assert_eq!(run_cache.misses(), 1);

    // Warm pass: same sources. The run cache should serve the
    // entire output — no per-file work runs at all.
    let t_warm = Instant::now();
    let (warm_issues, _) = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    let warm_ns = t_warm.elapsed().as_nanos();

    assert_eq!(
        cold_issues.len(),
        warm_issues.len(),
        "warm pass must produce the same findings count as cold"
    );
    assert_eq!(
        run_cache.hits(),
        1,
        "warm pass must register exactly one RunCache hit"
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
        ts_files: cold_issues.len(),
        cold_ms,
        warm_ms,
        speedup_ratio,
        run_cache_hits: run_cache.hits(),
        run_cache_misses: run_cache.misses(),
        acceptance_gate: ACCEPTANCE_GATE,
        hard_floor: HARD_FLOOR,
    };

    println!();
    println!("[run_cache_bench] results");
    println!("  repo:                  {}", result.repo);
    println!("  cold issues:           {}", result.ts_files);
    println!("  cold:                  {:>10.2} ms", cold_ms);
    println!("  warm:                  {:>10.2} ms", warm_ms);
    println!(
        "  speedup:               {:>10.2}× (gate {:.1}×, floor {:.1}×)",
        speedup_ratio, ACCEPTANCE_GATE, HARD_FLOOR
    );
    println!(
        "  run cache hits/misses: {} / {}",
        result.run_cache_hits, result.run_cache_misses
    );
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

    assert!(
        speedup_ratio >= HARD_FLOOR,
        "[run_cache_bench] regression: speedup {:.2}× < {:.1}× floor (acceptance gate {:.1}×); \
         cold={:.2}ms warm={:.2}ms",
        speedup_ratio,
        HARD_FLOOR,
        ACCEPTANCE_GATE,
        cold_ms,
        warm_ms
    );
}
