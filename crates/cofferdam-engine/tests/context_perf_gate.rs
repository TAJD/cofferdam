//! CD-164 (CP8) — `cofferdam context` performance gate.
//!
//! Synthesizes a 5,000-file TypeScript corpus (not checked in — generated
//! at test time so the repo doesn't carry a multi-MB fixture tree) and
//! times `Engine::analyze_context` (builtins + all Context providers)
//! against the epic's spec targets: warm-cache p50 <= 2s, p95 <= 5s.
//!
//! "Warm-cache" here means the corpus is already read into memory and the
//! OS filesystem cache is warm from the untimed setup pass — matching
//! `corpus_bench.rs`'s existing precedent (see that file's module doc for
//! why the perf assertion lives behind `#[ignore]` rather than in the
//! default `cargo test` run: it must never flake the pre-push hook on a
//! slow CI runner, and is run explicitly instead).
//!
//! Run explicitly: `cargo test --release -p cofferdam-engine --test context_perf_gate -- --ignored --nocapture`

use std::path::PathBuf;
use std::time::Instant;

use cofferdam_checks::{all_builtins, all_context_providers};
use cofferdam_core::ChangeSet;
use cofferdam_engine::Engine;
use tempfile::TempDir;

const FILE_COUNT: usize = 5_000;
/// Every Nth file imports the previous one, so the canonical graph and
/// `Context.BlastRadius`/`Context.Annotations`' import-edge lookups have
/// real edges to walk rather than timing an all-isolated-files best case.
const IMPORT_STRIDE: usize = 10;
const WARM_ITERATIONS: usize = 15;
const P50_MS_LIMIT: u128 = 2_000;
const P95_MS_LIMIT: u128 = 5_000;

fn synthesize_corpus() -> (TempDir, Vec<(PathBuf, String)>) {
    let tmp = TempDir::new().expect("temp dir");
    let mut sources = Vec::with_capacity(FILE_COUNT);
    for i in 0..FILE_COUNT {
        let path = tmp.path().join(format!("f{i}.ts"));
        let text = if i > 0 && i % IMPORT_STRIDE == 0 {
            format!(
                "import {{ value{prev} }} from './f{prev}';\n\
                 // @cofferdam-context: derived from f{prev}, keep in sync\n\
                 export const value{i} = value{prev} + 1;\n",
                prev = i - 1,
                i = i
            )
        } else {
            format!("export const value{i} = {i};\n")
        };
        std::fs::write(&path, &text).expect("write synthetic fixture");
        sources.push((path, text));
    }
    (tmp, sources)
}

fn percentile(sorted_ms: &[u128], pct: f64) -> u128 {
    let idx = ((sorted_ms.len() as f64 - 1.0) * pct).round() as usize;
    sorted_ms[idx]
}

#[test]
#[ignore]
fn context_warm_cache_meets_p50_p95_targets_on_5k_file_corpus() {
    let (_tmp, sources) = synthesize_corpus();
    let last = sources.last().expect("non-empty corpus").0.clone();
    let changeset = ChangeSet::from_files([last]);

    let mut checks = all_builtins();
    checks.extend(all_context_providers());
    let engine = Engine::new(checks);

    // Untimed warm-up: primes the OS filesystem cache and pays any
    // one-time allocator/thread-pool setup cost outside the measured
    // window.
    let _ = engine.analyze_context(sources.clone(), &changeset);

    let mut durations_ms: Vec<u128> = Vec::with_capacity(WARM_ITERATIONS);
    for _ in 0..WARM_ITERATIONS {
        let start = Instant::now();
        let out = engine.analyze_context(sources.clone(), &changeset);
        durations_ms.push(start.elapsed().as_millis());
        assert!(
            !out.items.is_empty(),
            "expected the synthetic corpus's import/annotation edges to produce context items"
        );
    }
    durations_ms.sort_unstable();

    let p50 = percentile(&durations_ms, 0.50);
    let p95 = percentile(&durations_ms, 0.95);
    println!("context perf gate: p50={p50}ms p95={p95}ms samples={durations_ms:?}");

    assert!(
        p50 <= P50_MS_LIMIT,
        "p50 {p50}ms exceeds target {P50_MS_LIMIT}ms (samples: {durations_ms:?})"
    );
    assert!(
        p95 <= P95_MS_LIMIT,
        "p95 {p95}ms exceeds target {P95_MS_LIMIT}ms (samples: {durations_ms:?})"
    );
}
