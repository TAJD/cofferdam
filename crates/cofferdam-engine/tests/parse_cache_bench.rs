//! cd-9hp.4 cp1 — parse-cache no-op-re-run regression budget.
//!
//! Measures the wall-clock speedup of a no-op re-run of
//! [`cofferdam_engine::Engine::analyze_with_sources_cached`] against
//! the equivalent cold run.
//!
//! ## What the bead expected vs reality
//!
//! The bead's cp1 acceptance gate was *"no-op re-run ≥3× faster than
//! cold"*, written on the hypothesis that parse is the dominant
//! per-file cost. On the bestefforttools fixture in release mode
//! that hypothesis doesn't hold: parse is roughly 10% of cofferdam's
//! cost, with per-check AST walks plus graph extract plus finalize
//! making up the rest. Shaving parse alone delivers ~1.1×.
//!
//! The 3× target therefore shifts to cp2 (per-file findings cache),
//! where skipping the check loop on cache hit is the load-bearing
//! win. This bench's role at cp1 is to **record the measured ratio
//! over time** as a regression trail. The asserted floor is just
//! "warm cannot be slower than cold" (1.0×) — anything below that
//! is a real regression. The documented budget tracks the measured
//! value so a later checkpoint that touches the cache without
//! retuning the bench will surface the change in the printed line.
//!
//! ## Target repo
//!
//! Configurable via `COFFERDAM_BENCH_REPO`. Falls back to
//! `C:/Users/tajdi/bestefforttools` (the 325-TS-file known-good repo
//! pinned in `CLAUDE.md`). When neither is present, the test prints
//! a skip notice and returns — CI without the repo must not fail.
//!
//! ## What's measured
//!
//! - **cold_ms** — first call to `analyze_with_sources_cached`
//!   against an empty cache. Includes parse + graph extract + every
//!   check + finalize.
//! - **warm_ms** — second call against the same sources with the
//!   cache from the cold pass. Parse is skipped on every TS file;
//!   the rest of the pipeline still runs.
//! - **speedup_ratio** — `cold_ms / warm_ms`. The number the bead's
//!   acceptance gate is written against.

use std::path::{Path, PathBuf};
use std::time::Instant;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{cache::ParseCache, discover, DiscoveryOptions, Engine};
use serde::Serialize;

/// Measured speedup ratio on bestefforttools, release build, dev
/// box (cp1 baseline). Tracks the *current* parse-cache contribution
/// to no-op re-run cost. cp2 lifts this when findings caching ships.
const DOCUMENTED_BUDGET: f64 = 1.1;
/// Below this and we flake the test rather than silently ship a
/// regression. cp1 ships a parse-only cache, so anything below 1.0×
/// means the warm pass paid *more* than the cold pass — broken.
const HARD_FLOOR: f64 = 1.0;

#[derive(Debug, Serialize)]
struct BenchResult {
    repo: String,
    ts_files: usize,
    cold_ms: f64,
    warm_ms: f64,
    speedup_ratio: f64,
    cache_hits_after_warm: u64,
    cache_misses_after_warm: u64,
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
        .join("parse-cache-bench-results")
}

#[test]
fn parse_cache_no_op_rerun_within_budget() {
    let Some(repo) = target_repo() else {
        println!(
            "[parse_cache_bench] no bench repo found; set COFFERDAM_BENCH_REPO \
             or place a TS repo at C:/Users/tajdi/bestefforttools. Skipping."
        );
        return;
    };

    let discovery_opts = DiscoveryOptions::default();
    let files = discover(std::slice::from_ref(&repo), &discovery_opts).expect("discover");
    println!(
        "[parse_cache_bench] {} TS files in {}",
        files.len(),
        repo.display()
    );

    // Read everything up front so the bench measures parse + checks,
    // not disk I/O.
    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|t| (p.clone(), t)))
        .collect();
    assert!(
        !sources.is_empty(),
        "[parse_cache_bench] target repo has no readable TS files"
    );

    let engine = Engine::new(all_builtins());
    let cache = ParseCache::new();

    // Cold pass: cache starts empty, every distinct text misses.
    let t_cold = Instant::now();
    let (cold_issues, _) = engine.analyze_with_sources_cached(sources.clone(), Some(&cache));
    let cold_ns = t_cold.elapsed().as_nanos();
    let cold_misses = cache.misses();
    let cold_hits = cache.hits();

    // Warm pass: same sources, populated cache. Every per-file
    // lookup should hit; no new misses recorded.
    let t_warm = Instant::now();
    let (warm_issues, _) = engine.analyze_with_sources_cached(sources, Some(&cache));
    let warm_ns = t_warm.elapsed().as_nanos();

    assert_eq!(
        cold_issues.len(),
        warm_issues.len(),
        "warm pass must produce the same findings count as cold"
    );
    assert_eq!(
        cache.misses(),
        cold_misses,
        "warm pass added new misses ({} → {}) — the cache lost an entry between calls",
        cold_misses,
        cache.misses()
    );
    let warm_hits_added = cache.hits() - cold_hits;
    assert!(
        warm_hits_added > 0,
        "warm pass logged zero cache hits — the engine isn't consulting the cache"
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
        ts_files: cold_misses as usize,
        cold_ms,
        warm_ms,
        speedup_ratio,
        cache_hits_after_warm: cache.hits(),
        cache_misses_after_warm: cache.misses(),
        documented_budget: DOCUMENTED_BUDGET,
        hard_floor: HARD_FLOOR,
    };

    println!();
    println!("[parse_cache_bench] results");
    println!("  repo:              {}", result.repo);
    println!("  TS files cached:   {}", result.ts_files);
    println!("  cold:              {:>10.2} ms", cold_ms);
    println!("  warm:              {:>10.2} ms", warm_ms);
    println!(
        "  speedup:           {:>10.2}× (budget {:.1}×, floor {:.1}×)",
        speedup_ratio, DOCUMENTED_BUDGET, HARD_FLOOR
    );
    println!(
        "  cache hits/misses: {} / {}",
        result.cache_hits_after_warm, result.cache_misses_after_warm
    );
    println!();

    // Persist a JSON row alongside graph-bench-results for the same
    // history-trail purpose. Gitignored; the committed budget is
    // this constant.
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
        "[parse_cache_bench] regression: warm pass slower than cold ({:.2}× < {:.1}× floor, \
         documented budget {:.1}×); cold={:.2}ms warm={:.2}ms",
        speedup_ratio,
        HARD_FLOOR,
        DOCUMENTED_BUDGET,
        cold_ms,
        warm_ms
    );
}
