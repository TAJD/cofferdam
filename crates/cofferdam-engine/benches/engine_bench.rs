//! CD-35 — criterion benches locking in the CD-28 epic's performance
//! wins (CD-29 pass-2 parse reuse, CD-30 rayon parallel run loop) so
//! later changes surface a regression on every PR instead of decaying
//! silently.
//!
//! Fixture corpus is `examples/` (checked into the repo, so this runs
//! identically on every CI runner — no dependency on a local
//! machine's vendored repos). Run locally with:
//!
//!   cargo bench -p cofferdam-engine
//!
//! Two benchmarks, matching the epic's two acceptance-gate claims:
//!
//! - `full_run_no_cache` — a cold, from-scratch `analyze_with_sources`
//!   pass over the whole corpus (no `ParseCache`). This is the
//!   N-core-scaling instrument: CD-30's rayon run loop is exercised
//!   here since `Engine::analyze_with_sources_cached(.., None)` takes
//!   the parallel path (see `lib.rs`'s module doc comment).
//! - `single_file_edit_incremental` — re-analyze after mutating
//!   exactly one file via `Engine::analyze_incremental`, against an
//!   `AnalysisState` pre-seeded (untimed) on the full corpus. This is
//!   the "O(changed files)" instrument: only the edited file re-runs
//!   pass 1; unchanged files reuse their cached pass-1 issues and hit
//!   the parse cache. Compared against `full_run_no_cache` it shows
//!   the incremental speedup the CD-28 epic promised.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{discover, AnalysisState, DiscoveryOptions, Engine};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// Corpus root to bench over. Defaults to `examples/` at the
/// workspace root (checked in, so CI is reproducible); override with
/// `COFFERDAM_BENCH_CORPUS=<dir>` to point the same benches at a
/// larger real-world tree for a representative incremental-vs-full
/// ratio.
fn examples_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("COFFERDAM_BENCH_CORPUS") {
        return PathBuf::from(dir)
            .canonicalize()
            .expect("COFFERDAM_BENCH_CORPUS must point at an existing directory");
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .canonicalize()
        .expect("examples/ must exist at the workspace root")
}

/// Discover and read every fixture `.ts` file under `examples/` once.
fn load_corpus() -> Vec<(PathBuf, String)> {
    let root = examples_dir();
    let files = discover(&[&root], &DiscoveryOptions::default())
        .expect("discovery over examples/ must not fail");
    assert!(
        !files.is_empty(),
        "examples/ produced no fixtures — bench corpus is empty"
    );
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            (path, text)
        })
        .collect()
}

fn bench_full_run_no_cache(c: &mut Criterion) {
    let corpus = load_corpus();
    let engine = Engine::new(all_builtins());

    c.bench_function("full_run_no_cache", |b| {
        b.iter_batched(
            || corpus.clone(),
            |sources| engine.analyze_with_sources_cached(sources, None),
            BatchSize::LargeInput,
        );
    });
}

fn bench_single_file_edit_incremental(c: &mut Criterion) {
    let corpus = load_corpus();
    let engine = Engine::new(all_builtins());

    // Each batch seeds a fresh AnalysisState over the whole corpus
    // (untimed), then the timed routine feeds `analyze_incremental`
    // exactly one edited file — so the measurement is the cost of
    // re-analyzing after a single edit given warm state, not a full
    // re-run. A unique marker per batch guarantees the edited file is
    // genuinely re-parsed rather than served from the parse cache.
    let edit_counter = AtomicUsize::new(0);

    c.bench_function("single_file_edit_incremental", |b| {
        b.iter_batched(
            || {
                let mut state = AnalysisState::new();
                engine.analyze_incremental(&mut state, &corpus, &[]);
                let n = edit_counter.fetch_add(1, Ordering::Relaxed);
                let (path, text) = corpus.first().expect("non-empty corpus").clone();
                let edited = (path, format!("{text}\n// bench-edit-{n}\n"));
                (state, edited)
            },
            |(mut state, edited)| {
                engine.analyze_incremental(&mut state, std::slice::from_ref(&edited), &[])
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_full_run_no_cache, bench_single_file_edit_incremental
}
criterion_main!(benches);
