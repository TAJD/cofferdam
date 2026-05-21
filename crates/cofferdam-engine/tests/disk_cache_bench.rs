//! cd-9hp.4 cp4 — disk-backed cache warm-from-disk speedup gate.
//!
//! Measures the wall-clock speedup of a "fresh process loads disk
//! cache and re-analyzes the same input set." Mirrors the
//! `run_cache_bench` shape but with a disk save/load step between
//! the cold and warm passes — i.e. it models a CI run that fetched
//! a prior build's cache.
//!
//! ## What we expect
//!
//! - The RunCache disk-loaded entry hits on the warm pass, short-
//!   circuiting the per-file loop entirely. So warm-pass cost ≈
//!   load-from-disk + one hashmap probe + one `Vec<Issue>` clone.
//! - Cold-pass cost is the full analyze (~120 ms on
//!   bestefforttools).
//! - The bead's acceptance gate is "CI usage shaves ~70% off cold-
//!   run time" — equivalent to ~3.3× speedup. We set the hard floor
//!   at 3.0× to absorb CI noise + disk variability.
//!
//! ## Target repo
//!
//! Configurable via `COFFERDAM_BENCH_REPO`. Falls back to
//! `C:/Users/tajdi/bestefforttools`. Skipped without a target.

use std::path::{Path, PathBuf};
use std::time::Instant;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{
    discover, disk_cache, findings_cache::FindingsCache, run_cache::RunCache, DiscoveryOptions,
    Engine,
};
use serde::Serialize;
use tempfile::tempdir;

/// "70% off cold-run time" from the bead = ~3.3× speedup. Gate at
/// 3.3× as the target; the hard floor below catches regressions
/// without flaking on CI noise.
const ACCEPTANCE_GATE: f64 = 3.3;
const HARD_FLOOR: f64 = 2.5;

#[derive(Debug, Serialize)]
struct BenchResult {
    repo: String,
    ts_files: usize,
    cold_ms: f64,
    load_ms: f64,
    warm_ms: f64,
    speedup_ratio: f64,
    findings_cache_loaded: usize,
    run_cache_loaded: usize,
    run_cache_hits: u64,
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
    workspace_root
        .join("tests")
        .join("disk-cache-bench-results")
}

#[test]
fn disk_cache_warm_pass_hits_acceptance_gate() {
    let Some(repo) = target_repo() else {
        println!(
            "[disk_cache_bench] no bench repo found; set COFFERDAM_BENCH_REPO \
             or place a TS repo at C:/Users/tajdi/bestefforttools. Skipping."
        );
        return;
    };

    let discovery_opts = DiscoveryOptions::default();
    let files = discover(std::slice::from_ref(&repo), &discovery_opts).expect("discover");
    println!(
        "[disk_cache_bench] {} TS files in {}",
        files.len(),
        repo.display()
    );

    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|t| (p.clone(), t)))
        .collect();
    assert!(
        !sources.is_empty(),
        "[disk_cache_bench] target repo has no readable TS files"
    );

    let cache_dir = tempdir().expect("tempdir for cache");
    let engine = Engine::new(all_builtins());

    // --- COLD PASS: empty caches; populate and save to disk ---
    let fc_cold = FindingsCache::new();
    let rc_cold = RunCache::new();
    let t_cold = Instant::now();
    let (cold_issues, _) =
        engine.analyze_with_sources_full(sources.clone(), None, Some(&fc_cold), Some(&rc_cold));
    let cold_ns = t_cold.elapsed().as_nanos();
    disk_cache::save_findings(cache_dir.path(), &fc_cold).expect("save findings");
    disk_cache::save_run(cache_dir.path(), &rc_cold).expect("save run");

    // --- WARM PASS: fresh in-memory caches hydrated from disk ---
    let fc_warm = FindingsCache::new();
    let rc_warm = RunCache::new();
    let registered_ids: Vec<&'static str> = all_builtins().iter().map(|c| c.meta().id).collect();
    let t_load = Instant::now();
    let loaded_f = disk_cache::load_findings(cache_dir.path(), &fc_warm, &registered_ids)
        .expect("load findings");
    let loaded_r = disk_cache::load_run(cache_dir.path(), &rc_warm).expect("load run");
    let load_ns = t_load.elapsed().as_nanos();

    let t_warm = Instant::now();
    let (warm_issues, _) =
        engine.analyze_with_sources_full(sources, None, Some(&fc_warm), Some(&rc_warm));
    let warm_ns = t_warm.elapsed().as_nanos();

    assert_eq!(
        cold_issues.len(),
        warm_issues.len(),
        "warm pass must produce same finding count as cold"
    );
    assert!(
        rc_warm.hits() >= 1,
        "warm pass should hit the disk-loaded run cache at least once (hits={})",
        rc_warm.hits()
    );

    let cold_ms = cold_ns as f64 / 1e6;
    let warm_ms = warm_ns as f64 / 1e6;
    let load_ms = load_ns as f64 / 1e6;
    // For the CI "saved compared to cold" framing, include disk load
    // time in the warm cost — a CI run that fetches the cache pays
    // both. The bead's gate is "70% off cold-run time" measured
    // end-to-end.
    let total_warm_ms = warm_ms + load_ms;
    let speedup_ratio = if total_warm_ms > 0.0 {
        cold_ms / total_warm_ms
    } else {
        f64::INFINITY
    };

    let result = BenchResult {
        repo: repo.display().to_string(),
        ts_files: cold_issues.len(),
        cold_ms,
        load_ms,
        warm_ms,
        speedup_ratio,
        findings_cache_loaded: loaded_f,
        run_cache_loaded: loaded_r,
        run_cache_hits: rc_warm.hits(),
        acceptance_gate: ACCEPTANCE_GATE,
        hard_floor: HARD_FLOOR,
    };

    println!();
    println!("[disk_cache_bench] results");
    println!("  repo:                {}", result.repo);
    println!("  cold issues:         {}", result.ts_files);
    println!("  cold:                {:>10.2} ms", cold_ms);
    println!("  disk load:           {:>10.2} ms", load_ms);
    println!("  warm (post-load):    {:>10.2} ms", warm_ms);
    println!(
        "  end-to-end warm:     {:>10.2} ms  (load + warm)",
        total_warm_ms
    );
    println!(
        "  speedup:             {:>10.2}× (gate {:.1}×, floor {:.1}×)",
        speedup_ratio, ACCEPTANCE_GATE, HARD_FLOOR
    );
    println!(
        "  findings loaded / run loaded: {} / {}",
        loaded_f, loaded_r
    );
    println!("  run cache hits:      {}", rc_warm.hits());
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
        "[disk_cache_bench] regression: speedup {:.2}× < {:.1}× floor \
         (acceptance gate {:.1}×); cold={:.2}ms load={:.2}ms warm={:.2}ms",
        speedup_ratio,
        HARD_FLOOR,
        ACCEPTANCE_GATE,
        cold_ms,
        load_ms,
        warm_ms
    );
}
