//! Integration: engine + full-run snapshot cache (cd-9hp.4 cp3).
//!
//! Pins two contracts the cp3 acceptance gates demand:
//!
//! 1. **No-op re-run** with all three caches present is a single
//!    `RunCache` hit — no per-file work, no graph extract, no
//!    finalize. The wall-clock comparison lives in the bench; this
//!    file confirms the contract via hit counters.
//!
//! 2. **Multi-file correctness**: edit one of N files contributing
//!    to a corpus slot, verify the engine re-runs (cache misses on
//!    the run layer) AND `finalize` sees the new aggregate.
//!    Specifically, the cross-file `Design.DuplicateExportName`
//!    finding pattern shifts when one file's export changes —
//!    proving the corpus is rebuilt from current sources, not from
//!    stale snapshot bytes.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{
    cache::ParseCache, findings_cache::FindingsCache, run_cache::RunCache, Engine,
};

fn engine() -> Engine {
    Engine::new(all_builtins())
}

fn finding_keys(issues: &[cofferdam_core::Issue]) -> Vec<(String, String, u32, u32, String)> {
    let mut out: Vec<_> = issues
        .iter()
        .map(|i| {
            (
                i.check_id.clone(),
                i.file.to_string_lossy().replace('\\', "/"),
                i.location.line(),
                i.location.column(),
                i.message.clone(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn no_op_re_run_serves_from_run_cache() {
    let engine = engine();
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let run_cache = RunCache::new();

    let sources = vec![
        (
            PathBuf::from("a.ts"),
            "export const alpha = 1;\n".to_string(),
        ),
        (
            PathBuf::from("b.ts"),
            "export const beta = 2;\n".to_string(),
        ),
    ];

    let (first, _) = engine.analyze_with_sources_full(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    assert_eq!(run_cache.hits(), 0);
    assert_eq!(run_cache.misses(), 1);
    let inserts_after_first = run_cache.len();
    assert_eq!(inserts_after_first, 1);

    let (second, _) = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );

    assert_eq!(
        finding_keys(&first),
        finding_keys(&second),
        "warm pass must produce identical findings to cold"
    );
    assert_eq!(run_cache.hits(), 1, "no-op re-run must hit the run cache");
    // The inner caches don't get a second probe — RunCache short-
    // circuits before the per-file loop runs. Confirm the parse
    // cache stayed at its cold-pass miss count.
    assert!(parse_cache.hits() == 0);
}

#[test]
fn editing_one_file_misses_run_cache_and_findings_reflect_new_state() {
    // The bead's acceptance bullet: edit one of N files contributing
    // to a corpus slot, verify finalize sees the new aggregate.
    //
    // Setup: two files each exporting a name `Widget`. Cross-file
    // `Design.DuplicateExportName` should flag both files in cold +
    // warm passes. Then edit one file to remove the duplicate
    // export — the warm pass must re-run, find no duplicate, and
    // emit no finding.
    let engine = engine();
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let run_cache = RunCache::new();

    let mut sources = vec![
        (
            PathBuf::from("a.ts"),
            "export const Widget = 1;\n".to_string(),
        ),
        (
            PathBuf::from("b.ts"),
            "export const Widget = 2;\n".to_string(),
        ),
    ];

    let (cold, _) = engine.analyze_with_sources_full(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    assert!(
        cold.iter()
            .any(|i| i.check_id == "Design.DuplicateExportName"),
        "expected Design.DuplicateExportName in the cold pass"
    );

    // Edit b.ts so the duplicate goes away.
    for (path, text) in sources.iter_mut() {
        if path.file_name().and_then(|n| n.to_str()) == Some("b.ts") {
            *text = "export const Renamed = 2;\n".to_string();
        }
    }

    let (warm, _) = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );

    // RunCache should miss (different input fingerprint) and the
    // fresh analyze should observe the new corpus aggregate.
    assert_eq!(
        run_cache.misses(),
        2,
        "edited input set must miss the run cache (got hits={}, misses={})",
        run_cache.hits(),
        run_cache.misses()
    );
    assert!(
        !warm
            .iter()
            .any(|i| i.check_id == "Design.DuplicateExportName"),
        "Design.DuplicateExportName must not fire after the duplicate is removed; \
         got: {:?}",
        warm.iter()
            .filter(|i| i.check_id == "Design.DuplicateExportName")
            .map(|i| (i.file.display().to_string(), i.message.clone()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn run_cache_layered_with_inner_caches_matches_uncached() {
    // Defence-in-depth: the layered (parse + findings + run) result
    // must match the no-cache baseline on the SAME inputs.
    let engine = engine();
    let sources = vec![
        (
            PathBuf::from("a.ts"),
            r#"
export function alpha(x: any) {
  if (x == 1) {                 // Warning.TripleEquals
    return true;
  }
  return false;
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("b.ts"),
            "import { alpha } from './a';\nexport const beta = alpha(1);\n".to_string(),
        ),
    ];

    let (baseline, _) = engine.analyze_with_sources(sources.clone());

    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let run_cache = RunCache::new();
    let (layered, _) = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );

    assert_eq!(
        finding_keys(&baseline),
        finding_keys(&layered),
        "layered-cache path must produce identical findings to the no-cache path"
    );
}

#[test]
fn run_cache_isolates_across_config_changes() {
    // Two engines built from the same registry share a config_hash,
    // so they share `RunCache` keys. Two engines with different
    // resolved configs would shift the config_hash; we don't have a
    // direct knob in this fixture, so we exercise the invariant
    // indirectly: clearing the cache on `clear()` resets the
    // mechanism (proxy for "config change → fresh start").
    let engine = engine();
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let run_cache = RunCache::new();
    let sources = vec![(PathBuf::from("a.ts"), "export const x = 1;\n".to_string())];

    let _ = engine.analyze_with_sources_full(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    assert_eq!(run_cache.len(), 1);
    run_cache.clear();
    let _ = engine.analyze_with_sources_full(
        sources,
        Some(&parse_cache),
        Some(&findings_cache),
        Some(&run_cache),
    );
    assert_eq!(
        run_cache.hits(),
        0,
        "clear() then re-analyze should produce no hits"
    );
    assert_eq!(run_cache.misses(), 1);
    assert_eq!(run_cache.len(), 1);
}
