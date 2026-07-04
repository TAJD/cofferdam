//! Integration: rayon-parallel run loop matches the sequential path
//! (CD-30).
//!
//! `analyze_with_sources` (no `ParseCache` supplied) takes the
//! rayon-parallel dispatch in `analyze_with_sources_caches_inner`;
//! `analyze_with_sources_cached(..., Some(&ParseCache::new()))` takes
//! the sequential, single-threaded dispatch that reuses one parse
//! across pass 1 and pass 2 (CD-29). Both must produce byte-identical
//! findings — same content, same order — over every fixture in
//! `examples/`, which exercises the full built-in check set (per-file
//! checks, cross-file corpus checks, consistency/pass2 checks, and
//! the canonical-graph finalize pass) across many files at once.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{cache::ParseCache, Engine};

fn engine() -> Engine {
    Engine::new(all_builtins())
}

/// Every `.ts` fixture directly under `examples/` (top-level only —
/// the subdirectories there are self-contained multi-file fixtures
/// with their own path-relationship expectations, out of scope for a
/// flat determinism sweep).
fn example_sources() -> Vec<(PathBuf, String)> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let examples_dir = manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("examples");

    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", examples_dir.display()))
    {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ts") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
            sources.push((path, text));
        }
    }
    assert!(!sources.is_empty(), "expected at least one .ts fixture");
    sources
}

/// Full identity of one issue — file, check, exact location, message,
/// severity, priority — so a mismatch anywhere (including a shifted
/// sort order the two paths might disagree on) fails loudly.
fn full_identity(issues: &[cofferdam_core::Issue]) -> Vec<String> {
    issues
        .iter()
        .map(|i| {
            format!(
                "{}|{}|{}:{}|{:?}|{:?}|{}",
                i.check_id,
                i.file.to_string_lossy().replace('\\', "/"),
                i.location.line(),
                i.location.column(),
                i.severity,
                i.priority,
                i.message
            )
        })
        .collect()
}

#[test]
fn parallel_run_matches_sequential_run_byte_identical() {
    let sources = example_sources();

    let sequential_engine = engine();
    let parse_cache = ParseCache::new();
    let (sequential, _) =
        sequential_engine.analyze_with_sources_cached(sources.clone(), Some(&parse_cache));

    let parallel_engine = engine();
    let (parallel, _) = parallel_engine.analyze_with_sources(sources);

    assert_eq!(
        full_identity(&sequential),
        full_identity(&parallel),
        "rayon-parallel run loop (CD-30) must match the sequential, \
         cached-parse run loop (CD-29) exactly — including order"
    );
}

#[test]
fn parallel_run_is_stable_across_repeated_calls() {
    // Same engine, same sources, run through the parallel path twice.
    // Rayon's work-stealing scheduler assigns files to threads
    // non-deterministically call to call — this pins that the final
    // sorted output is nonetheless identical every time.
    let sources = example_sources();
    let engine = engine();

    let (first, _) = engine.analyze_with_sources(sources.clone());
    let (second, _) = engine.analyze_with_sources(sources);

    assert_eq!(
        full_identity(&first),
        full_identity(&second),
        "repeated parallel runs over the same sources must agree byte-for-byte"
    );
}
