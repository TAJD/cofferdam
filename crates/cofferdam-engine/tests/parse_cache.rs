//! Integration: engine + parse cache (cd-9hp.4 cp1).
//!
//! Pins the cached path's behaviour against the non-cached path: the
//! same sources analyzed twice through `analyze_with_sources_cached`
//! must produce byte-identical findings, and the second run must
//! register cache hits proving parse work was skipped.
//!
//! This test does NOT measure wall-clock speedup — that's the bench
//! suite's job. It DOES guarantee correctness, which is the
//! load-bearing contract for everything cp2+ layers on top.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Category, Check, CheckContext, CheckMeta, Issue, Severity, SourceFile};
use cofferdam_engine::{cache::ParseCache, Engine};

/// `all_builtins()` ships no `consistency: true` (two-pass) check today,
/// so nothing in it ever re-reads a file's cached parse on a pass2
/// lookup — the only path that registers a `ParseCache` hit. This no-op
/// two-pass check exists purely to exercise that path so this suite's
/// hit-count assertions still mean something (CD-357 pass 3 removed
/// `Consistency.QuoteStyle`, the last real one).
struct NoopConsistencyCheck;

const NOOP_META: CheckMeta = CheckMeta {
    id: "Test.NoopConsistency",
    category: Category::Consistency,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "test-only two-pass check to exercise the parse-cache pass2 hit path",
    body: "test-only two-pass check to exercise the parse-cache pass2 hit path",
    requires_types: false,
    consistency: true,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for NoopConsistencyCheck {
    fn meta(&self) -> &'static CheckMeta {
        &NOOP_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn pass2(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
}

/// A small, deliberately-orphaned TS project. Exercises both
/// per-file checks (Warning.TripleEquals, Refactor.UnusedVariable)
/// and the cross-file graph path (Design.OrphanExport via
/// CANONICAL_GRAPH).
fn project_sources() -> Vec<(PathBuf, String)> {
    vec![
        (
            PathBuf::from("a.ts"),
            r#"
export function alpha(x: any) {
  if (x == 1) {           // triggers Warning.TripleEquals
    return true;
  }
  const unused = 42;       // triggers Refactor.UnusedVariable
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
            // Pure orphan exporter — nothing imports `gamma`, so the
            // canonical graph should report it via Design.OrphanExport.
            r#"
export const gamma = 99;
"#
            .to_string(),
        ),
    ]
}

fn engine() -> Engine {
    let mut checks = all_builtins();
    checks.push(Box::new(NoopConsistencyCheck));
    Engine::new(checks)
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

#[test]
fn cached_run_matches_uncached_run() {
    let engine = engine();

    let sources = project_sources();
    let (uncached_issues, _) = engine.analyze_with_sources(sources.clone());

    let cache = ParseCache::new();
    let (cached_issues, _) = engine.analyze_with_sources_cached(sources, Some(&cache));

    assert_eq!(
        finding_keys(&uncached_issues),
        finding_keys(&cached_issues),
        "cached path must produce the same findings as the non-cached path"
    );
    // Pass 1 populates the cache (a miss per TS file); pass 2's
    // consistency check (Consistency.QuoteStyle) reads the same
    // content hash back and hits instead of re-parsing (CD-29).
    assert_eq!(cache.hits(), 3);
    assert!(cache.misses() >= 3);
}

#[test]
fn second_run_is_all_cache_hits() {
    let engine = engine();
    let cache = ParseCache::new();

    let sources = project_sources();
    let ts_file_count = sources.len() as u64;

    let (first, _) = engine.analyze_with_sources_cached(sources.clone(), Some(&cache));
    // Pass 2's consistency check hits every entry pass 1 just
    // populated within this same call (CD-29).
    assert_eq!(cache.hits(), ts_file_count);
    assert_eq!(cache.misses(), ts_file_count);

    let (second, _) = engine.analyze_with_sources_cached(sources, Some(&cache));
    assert_eq!(
        finding_keys(&first),
        finding_keys(&second),
        "deterministic findings across runs"
    );
    assert_eq!(
        cache.hits(),
        ts_file_count * 3,
        "second run: pass 1 hits the first run's entries, pass 2 hits again — \
         on top of the first run's own pass-2 hits"
    );
    assert_eq!(
        cache.misses(),
        ts_file_count,
        "no new misses on the second run"
    );
}

#[test]
fn changed_file_invalidates_only_its_slot() {
    // Modify b.ts between runs. a.ts and c.ts retain their cache
    // entries (content unchanged → same hash); b.ts misses.
    let engine = engine();
    let cache = ParseCache::new();

    let mut sources = project_sources();
    let _ = engine.analyze_with_sources_cached(sources.clone(), Some(&cache));
    // Pass 2's consistency check hits every entry pass 1 just
    // populated within this same call (CD-29).
    assert_eq!(cache.hits(), 3);
    assert_eq!(cache.misses(), 3);

    // Replace b.ts body. The cache is content-keyed so this lands as
    // a new entry alongside the original (no removal pass at cp1 —
    // the dead entry stays until `clear` is called).
    let new_b = r#"
import { alpha } from './a';

// edited
export function beta(): boolean {
  return alpha(1) && alpha(2);
}
"#;
    for (path, text) in sources.iter_mut() {
        if path.file_name().and_then(|n| n.to_str()) == Some("b.ts") {
            *text = new_b.to_string();
        }
    }

    let _ = engine.analyze_with_sources_cached(sources, Some(&cache));
    // Second call: pass 1 hits a.ts + c.ts (unchanged) and misses
    // b.ts (new content-hash); pass 2 then hits all three (its
    // lookups always land after pass 1 has populated them this call —
    // CD-29). Cumulative: 3 (first call's pass 2) + 2 (pass 1) + 3
    // (pass 2) = 8 hits; 3 (first call) + 1 (b.ts) = 4 misses.
    assert_eq!(cache.hits(), 8);
    assert_eq!(cache.misses(), 4, "b.ts's edited body adds one miss");
    // The cache retains both b.ts entries (old + new); cp1 has no
    // path-keyed eviction.
    assert_eq!(cache.len(), 4);
}
