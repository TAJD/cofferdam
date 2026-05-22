//! Integration: engine + disk-backed findings/run cache (cd-9hp.4 cp4).
//!
//! Pins the cp4 acceptance gates:
//!
//! 1. **Cold-then-warm-from-disk** — analyze populates the in-memory
//!    cache; saving to disk and loading from a fresh cache then
//!    re-analyzing preserves findings byte-for-byte AND skips
//!    per-file work for pure checks (FindingsCache hits) plus the
//!    full-run cache for unchanged input sets (RunCache hits).
//!
//! 2. **Config-axis invalidation** — a config change shifts the
//!    config_hash; disk-cache entries from the prior config no
//!    longer match and the engine re-runs pure checks rather than
//!    silently replaying stale findings. (Pinned through the
//!    in-memory cache's existing `(content, config, check_id)` key
//!    — the disk layer is a transparent persistence tier.)
//!
//! 3. **Version isolation** — disk cache files are written under a
//!    `<engine_version>` subdir. A stray cache under a different
//!    version's subdir doesn't poison the active run.
//!
//! 4. **Corruption recovery** — a half-written or hand-edited cache
//!    file is treated as "no cache" and the run completes from
//!    cold.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{
    cache::ParseCache,
    disk_cache::{self, version_dir},
    findings_cache::{FindingsCache, ENGINE_VERSION},
    run_cache::RunCache,
    Engine,
};
use tempfile::tempdir;

fn engine() -> Engine {
    Engine::new(all_builtins())
}

fn finding_keys(issues: &[cofferdam_core::Issue]) -> Vec<(String, String, u32, u32)> {
    let mut out: Vec<_> = issues
        .iter()
        .map(|i| {
            (
                i.check_id.clone(),
                i.file.to_string_lossy().replace('\\', "/"),
                i.location.line(),
                i.location.column(),
            )
        })
        .collect();
    out.sort();
    out
}

fn registered_ids() -> Vec<&'static str> {
    all_builtins().iter().map(|c| c.meta().id).collect()
}

/// Round-trip: cold pass populates the caches, disk save persists,
/// fresh process (simulated by fresh in-memory caches) loads from
/// disk and serves the warm pass without re-running per-file work
/// for unchanged inputs.
#[test]
fn warm_pass_from_disk_matches_cold_pass() {
    let dir = tempdir().expect("tempdir");
    let engine = engine();
    let sources = vec![
        (
            PathBuf::from("a.ts"),
            "export const alpha = 1;\nif (alpha == 1) {}\n".to_string(),
        ),
        (
            PathBuf::from("b.ts"),
            "export const beta = 2;\n".to_string(),
        ),
    ];

    // --- COLD PASS ---
    let parse_cache_1 = ParseCache::new();
    let findings_cache_1 = FindingsCache::new();
    let run_cache_1 = RunCache::new();
    let (cold_issues, _) = engine.analyze_with_sources_full(
        sources.clone(),
        Some(&parse_cache_1),
        Some(&findings_cache_1),
        Some(&run_cache_1),
    );
    assert!(
        !findings_cache_1.is_empty(),
        "cold pass must populate findings cache"
    );
    assert!(!run_cache_1.is_empty(), "cold pass must populate run cache");

    let saved_findings = disk_cache::save_findings(dir.path(), &findings_cache_1).unwrap();
    let saved_run = disk_cache::save_run(dir.path(), &run_cache_1).unwrap();
    assert!(
        saved_findings > 0,
        "expected non-zero findings cache entries on disk"
    );
    assert_eq!(
        saved_run, 1,
        "run cache should have one entry per input set"
    );

    // --- SIMULATED FRESH PROCESS ---
    let findings_cache_2 = FindingsCache::new();
    let run_cache_2 = RunCache::new();
    let ids = registered_ids();
    let loaded_f = disk_cache::load_findings(dir.path(), &findings_cache_2, &ids).unwrap();
    let loaded_r = disk_cache::load_run(dir.path(), &run_cache_2).unwrap();
    assert_eq!(loaded_f, saved_findings);
    assert_eq!(loaded_r, saved_run);

    let parse_cache_2 = ParseCache::new();
    let (warm_issues, _) = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache_2),
        Some(&findings_cache_2),
        Some(&run_cache_2),
    );

    assert_eq!(
        finding_keys(&cold_issues),
        finding_keys(&warm_issues),
        "warm-from-disk pass must reproduce cold findings byte-for-byte"
    );
    assert_eq!(
        run_cache_2.hits(),
        1,
        "warm pass should hit the run cache once (same input set)"
    );
}

/// Editing one file flips the run-cache key; the engine misses the
/// outer cache and re-runs, but disk-loaded findings for the
/// unchanged file still hit the per-file findings cache. End state:
/// findings reflect the edit.
#[test]
fn editing_one_file_invalidates_run_cache_but_findings_cache_still_helps() {
    let dir = tempdir().expect("tempdir");
    let engine = engine();
    let mut sources = vec![
        (PathBuf::from("a.ts"), "export const x = 1;\n".to_string()),
        (PathBuf::from("b.ts"), "export const y = 2;\n".to_string()),
    ];

    // Cold pass.
    let fc1 = FindingsCache::new();
    let rc1 = RunCache::new();
    let _ = engine.analyze_with_sources_full(sources.clone(), None, Some(&fc1), Some(&rc1));
    disk_cache::save_findings(dir.path(), &fc1).unwrap();
    disk_cache::save_run(dir.path(), &rc1).unwrap();

    // Edit a.ts; b.ts unchanged.
    sources[0].1 = "export const x = 99;\nif (x == 99) {}\n".to_string();

    // Fresh in-memory caches hydrated from disk.
    let fc2 = FindingsCache::new();
    let rc2 = RunCache::new();
    disk_cache::load_findings(dir.path(), &fc2, &registered_ids()).unwrap();
    disk_cache::load_run(dir.path(), &rc2).unwrap();

    let (issues, _) = engine.analyze_with_sources_full(sources, None, Some(&fc2), Some(&rc2));

    // RunCache must MISS — the input set fingerprint changed.
    assert_eq!(
        rc2.misses(),
        1,
        "edited input set must miss the run cache (hits={}, misses={})",
        rc2.hits(),
        rc2.misses()
    );
    // FindingsCache must HIT for b.ts's pure checks (unchanged
    // content). The exact hit count depends on how many pure checks
    // register for TypeScript files, but at minimum we expect more
    // hits than zero.
    assert!(
        fc2.hits() > 0,
        "warm pass should serve unchanged file's pure checks from findings cache (hits={})",
        fc2.hits()
    );
    // Newly-edited file should have a TripleEquals finding.
    assert!(
        issues.iter().any(|i| i.check_id == "Warning.TripleEquals"
            && i.file.file_name().and_then(|n| n.to_str()) == Some("a.ts")),
        "expected Warning.TripleEquals on the edited file"
    );
}

/// Stray cache file under a non-matching engine version subdir must
/// not be picked up by a load from the canonical version subdir.
#[test]
fn version_subdir_isolates_caches() {
    let dir = tempdir().expect("tempdir");
    // Plant a bogus cache file under "0.0.0-other/findings.json".
    let bogus_dir = dir.path().join("0.0.0-other");
    std::fs::create_dir_all(&bogus_dir).unwrap();
    std::fs::write(
        bogus_dir.join("findings.json"),
        br#"{"engine_version":"0.0.0-other","entries":[]}"#,
    )
    .unwrap();

    // A load from the active version's subdir (which doesn't exist
    // yet) should hydrate zero entries.
    let fc = FindingsCache::new();
    let n = disk_cache::load_findings(dir.path(), &fc, &registered_ids()).unwrap();
    assert_eq!(n, 0, "version subdir mismatch must not hydrate");
    assert!(fc.is_empty());
}

/// A garbled findings.json on disk is silently discarded; the next
/// analyze runs cold and the next save overwrites the bad bytes.
#[test]
fn corrupted_cache_file_is_recovered_from() {
    let dir = tempdir().expect("tempdir");
    let vdir = version_dir(dir.path());
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(vdir.join("findings.json"), b"{ this is not valid json").unwrap();
    std::fs::write(vdir.join("run.json"), b"also garbage").unwrap();

    let fc = FindingsCache::new();
    let rc = RunCache::new();
    let f = disk_cache::load_findings(dir.path(), &fc, &registered_ids()).unwrap();
    let r = disk_cache::load_run(dir.path(), &rc).unwrap();
    assert_eq!(f, 0);
    assert_eq!(r, 0);

    // Now analyze cold + save; the file is overwritten.
    let engine = engine();
    let sources = vec![(PathBuf::from("a.ts"), "export const x = 1;\n".to_string())];
    let _ = engine.analyze_with_sources_full(sources, None, Some(&fc), Some(&rc));
    let saved_f = disk_cache::save_findings(dir.path(), &fc).unwrap();
    let saved_r = disk_cache::save_run(dir.path(), &rc).unwrap();
    assert!(saved_f > 0);
    assert_eq!(saved_r, 1);

    // Subsequent load should now succeed.
    let fc2 = FindingsCache::new();
    let n = disk_cache::load_findings(dir.path(), &fc2, &registered_ids()).unwrap();
    assert_eq!(n, saved_f);
}

/// The version_dir helper is correct: the on-disk layout includes
/// the current engine version so the disk-isolated-by-version
/// contract is verifiable.
#[test]
fn version_dir_uses_current_engine_version() {
    let root = PathBuf::from("/tmp/dummy");
    let v = version_dir(&root);
    assert!(v.ends_with(ENGINE_VERSION));
}
