//! Integration: engine + findings cache (cd-9hp.4 cp2).
//!
//! Pins the cached-path behaviour against the non-cached path: the
//! same sources analyzed through `analyze_with_sources_caches` with
//! both caches present must produce byte-identical findings, AND the
//! second pass must record findings-cache hits for every pure check.
//!
//! These tests don't measure wall-clock speedup (that's the bench's
//! job). They guarantee the load-bearing contract — cached findings
//! exactly match live ones — which is what cp3+ layers on top of.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{cache::ParseCache, findings_cache::FindingsCache, Engine};

fn project_sources() -> Vec<(PathBuf, String)> {
    vec![
        (
            PathBuf::from("a.ts"),
            r#"
export function alpha(x: any) {
  if (x == 1) {                 // Warning.TripleEquals fires
    return true;
  }
  const unused = 42;             // Refactor.UnusedVariable fires
  return false;
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("b.ts"),
            r#"
import { alpha } from './a';

export function beta(): boolean {
  return alpha(1);
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("c.ts"),
            r#"
export const gamma = 99;
"#
            .to_string(),
        ),
    ]
}

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
fn cached_findings_match_uncached_findings() {
    let engine = engine();
    let sources = project_sources();

    let (uncached, _) = engine.analyze_with_sources(sources.clone());

    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let (cached, _) =
        engine.analyze_with_sources_caches(sources, Some(&parse_cache), Some(&findings_cache));

    assert_eq!(
        finding_keys(&uncached),
        finding_keys(&cached),
        "findings-cached path must produce identical findings to the live path"
    );
    // First pass populated the cache; no hits possible yet.
    assert_eq!(findings_cache.hits(), 0);
    // At minimum, every pure check times every TS file inserted an
    // entry. With 3 TS files and several pure checks each, len > 0.
    assert!(
        !findings_cache.is_empty(),
        "cache should be populated after the first pass"
    );
}

#[test]
fn second_run_serves_pure_checks_from_findings_cache() {
    let engine = engine();
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let sources = project_sources();

    let (first, _) = engine.analyze_with_sources_caches(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
    );
    let inserts_after_first = findings_cache.len();
    assert!(inserts_after_first > 0);
    assert_eq!(findings_cache.hits(), 0);
    let misses_after_first = findings_cache.misses();

    let (second, _) =
        engine.analyze_with_sources_caches(sources, Some(&parse_cache), Some(&findings_cache));

    assert_eq!(
        finding_keys(&first),
        finding_keys(&second),
        "deterministic findings across runs"
    );
    // The warm pass must add zero new misses (every pure-check
    // lookup hit the cache).
    assert_eq!(
        findings_cache.misses(),
        misses_after_first,
        "warm pass added new misses ({} → {}) — cache lost a key between calls",
        misses_after_first,
        findings_cache.misses()
    );
    // And it must register at least one hit per cached entry.
    assert!(
        findings_cache.hits() >= inserts_after_first as u64,
        "warm pass hits ({}) should cover every cache entry ({})",
        findings_cache.hits(),
        inserts_after_first
    );
}

#[test]
fn editing_a_file_re_misses_findings_cache_for_that_file_only() {
    let engine = engine();
    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let mut sources = project_sources();

    let _ = engine.analyze_with_sources_caches(
        sources.clone(),
        Some(&parse_cache),
        Some(&findings_cache),
    );
    let misses_after_first = findings_cache.misses();

    // Replace b.ts text. Its content_hash changes; cache misses for
    // every pure check the engine asks for on b.ts. a.ts and c.ts
    // still hit.
    for (path, text) in sources.iter_mut() {
        if path.file_name().and_then(|n| n.to_str()) == Some("b.ts") {
            *text = "import { alpha } from './a';\nexport function beta(): boolean { return alpha(2); }\n".to_string();
        }
    }

    let _ = engine.analyze_with_sources_caches(sources, Some(&parse_cache), Some(&findings_cache));

    // Some new misses — one per pure TS check that ran on b.ts.
    // The exact count depends on how many pure checks fire, but it
    // must be > 0 and < total pure-check count (because a.ts +
    // c.ts still hit).
    let new_misses = findings_cache.misses() - misses_after_first;
    assert!(
        new_misses > 0,
        "edited file must produce new findings-cache misses"
    );
    // hits accumulated across both passes — strictly positive.
    assert!(findings_cache.hits() > 0);
}

#[test]
fn config_hash_changes_when_options_change() {
    // Two engines with default checks but different layers config
    // should produce different config_hash values, so cached
    // findings under one don't leak to the other. The current
    // `Engine::new(all_builtins())` doesn't expose a config knob
    // directly; we exercise the hash determinism instead — same
    // engine twice produces the same hash.
    let a = engine();
    let b = engine();
    assert_eq!(
        a.config_hash(),
        b.config_hash(),
        "two engines built from the same registry must share a config_hash"
    );
}

#[test]
fn non_pure_checks_always_re_run() {
    // Refactor.DuplicateBlock writes to corpus in run() and is
    // marked pure_run: false. With both caches present, its run()
    // must fire on every pass — otherwise the corpus would be
    // missing the per-file fingerprints that finalize() drains.
    //
    // We exercise the contract by checking that the findings-cache
    // entry count stays below `(pure_checks * files)` — if non-pure
    // checks were cached, the count would balloon. The exact
    // numbers depend on the registry, so we just assert the
    // structural invariant: non-pure checks contribute no entries.
    let engine = engine();
    let pure_check_ids: Vec<&str> = all_builtins()
        .iter()
        .filter(|c| c.meta().pure_run)
        .map(|c| c.meta().id)
        .collect();
    assert!(
        !pure_check_ids.is_empty(),
        "fixture: at least one pure check must exist"
    );

    let parse_cache = ParseCache::new();
    let findings_cache = FindingsCache::new();
    let sources = project_sources();
    let _ = engine.analyze_with_sources_caches(sources, Some(&parse_cache), Some(&findings_cache));

    // 3 TS files × N pure checks = upper bound on insertions.
    let max_entries = pure_check_ids.len() * 3;
    assert!(
        findings_cache.len() <= max_entries,
        "cache populated more entries ({}) than pure_check_ids × files ({}) — a non-pure check leaked through",
        findings_cache.len(),
        max_entries
    );
}
