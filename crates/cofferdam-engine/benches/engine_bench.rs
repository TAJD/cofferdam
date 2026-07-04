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
//! - `single_file_edit_incremental` — re-analyze the whole corpus
//!   after mutating exactly one file, against a `ParseCache` that was
//!   pre-warmed on the unmutated corpus. This is the "O(changed
//!   files)" instrument, today measuring only the parse-reuse slice of
//!   that claim (CD-29): corpus/graph/finalize still rebuild from
//!   scratch every call (see `cache.rs`'s module doc comment — "corpus
//!   state is still rebuilt from scratch every cycle").
//!   TODO(CD-32): switch to `Engine::analyze_incremental` once that
//!   API lands, so this bench measures the full incremental path
//!   rather than just parse-cache reuse.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use cofferdam_checks::all_builtins;
use cofferdam_engine::cache::ParseCache;
use cofferdam_engine::{discover, DiscoveryOptions, Engine};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// `examples/` at the workspace root, resolved relative to this
/// crate's manifest dir so the bench works regardless of the
/// caller's cwd.
fn examples_dir() -> PathBuf {
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

    // Pre-warm a cache against the unmutated corpus (untimed setup).
    let cache = ParseCache::new();
    engine.analyze_with_sources_cached(corpus.clone(), Some(&cache));

    // Each iteration edits the first file's text to a value the cache
    // has never seen, so the routine always pays exactly one parse
    // (the edited file) while every other file stays a cache hit.
    let edit_counter = AtomicUsize::new(0);

    c.bench_function("single_file_edit_incremental", |b| {
        b.iter_batched(
            || {
                let mut sources = corpus.clone();
                let n = edit_counter.fetch_add(1, Ordering::Relaxed);
                if let Some((_, text)) = sources.first_mut() {
                    text.push_str(&format!("\n// bench-edit-{n}\n"));
                }
                sources
            },
            |sources| engine.analyze_with_sources_cached(sources, Some(&cache)),
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
