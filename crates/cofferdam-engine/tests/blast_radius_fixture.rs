//! Fixture-repo integration test for `Context.BlastRadius` (CD-160).
//!
//! Exercises the acceptance criterion from the ticket / product spec
//! (success criterion 1): a synthetic exported-signature change puts
//! the affected direct callers in the digest top-3, deterministically.
//!
//! Reads real files under `examples/blast_radius/` (not virtual paths)
//! because import resolution goes through `oxc_resolver`, which needs
//! files that actually exist on disk.

use std::path::{Path, PathBuf};

use cofferdam_core::ChangeSet;
use cofferdam_engine::Engine;

fn fixture_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/cofferdam-engine; the fixture lives
    // at the workspace root's examples/blast_radius/. `.parent()` twice
    // (not `.join("..")` twice) — `std::path::absolute` only collapses
    // `..` components on Windows (GetFullPathNameW); on POSIX it's a
    // pure lexical CWD-prepend per the platform's own documented
    // semantics, so a literal `..` survives into the path used to key
    // graph nodes and never matches oxc_resolver's already-normalized
    // resolved paths — 0 incoming edges, 0 blast-radius items. CI-only
    // (Linux) failure root-caused via a temporary diagnostic test.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("blast_radius")
}

fn read_fixture_sources() -> Vec<(PathBuf, String)> {
    let dir = fixture_dir();
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read fixture file");
        // `std::path::absolute` (not `fs::canonicalize`) — canonicalize
        // adds the `\\?\` verbatim prefix on Windows, which the
        // engine's own resolver comparisons don't expect (see
        // `absolutize_sources` in cofferdam-engine::lib).
        let absolute = std::path::absolute(&path).expect("absolutize fixture path");
        sources.push((absolute, text));
    }
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
}

fn build_engine() -> Engine {
    let mut checks = cofferdam_checks::all_builtins();
    checks.extend(cofferdam_checks::all_context_providers());
    Engine::new(checks)
}

fn changed_lib_changeset(sources: &[(PathBuf, String)]) -> ChangeSet {
    let lib = sources
        .iter()
        .find(|(p, _)| p.file_name().and_then(|n| n.to_str()) == Some("lib.ts"))
        .expect("lib.ts present in fixture")
        .0
        .clone();
    ChangeSet::from_files([lib])
}

#[test]
fn blast_radius_surfaces_affected_callers_in_top_three() {
    let sources = read_fixture_sources();
    let changeset = changed_lib_changeset(&sources);
    let engine = build_engine();

    let out = engine.analyze_context(sources, &changeset);
    let blast_items: Vec<_> = out
        .items
        .iter()
        .filter(|i| i.check_id == "Context.BlastRadius")
        .collect();

    assert!(
        !blast_items.is_empty(),
        "expected Context.BlastRadius to surface at least one item"
    );

    // unrelated.ts must never be surfaced — it has no import-graph
    // path to lib.ts.
    assert!(
        blast_items
            .iter()
            .all(|i| !i.title.contains("unrelated.ts")),
        "unrelated.ts leaked into blast radius items: {blast_items:#?}"
    );

    // Top-3 by score must be the two depth-1 callers of the changed
    // export (direct_caller.ts, lib.test.ts) plus the depth-2
    // transitive consumer (wrapper.ts) — deep_consumer.ts (depth-3)
    // must rank below them.
    let mut sorted = blast_items.clone();
    sorted.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    let top3_titles: Vec<&str> = sorted.iter().take(3).map(|i| i.title.as_str()).collect();
    assert!(
        top3_titles.iter().any(|t| t.contains("direct_caller.ts")),
        "direct_caller.ts missing from top 3: {top3_titles:?}"
    );
    assert!(
        top3_titles.iter().any(|t| t.contains("lib.test.ts")),
        "lib.test.ts missing from top 3: {top3_titles:?}"
    );

    // Every item is traceable: explain is populated and mentions the
    // changed file.
    for item in &blast_items {
        let explain = item.explain.as_deref().unwrap_or("");
        assert!(
            explain.contains("changed lib.ts"),
            "item not traceable to the change: {item:?}"
        );
    }

    // The test file is annotated as such.
    let test_item = blast_items
        .iter()
        .find(|i| i.title.contains("lib.test.ts"))
        .expect("lib.test.ts item present");
    assert!(test_item.body.contains("test file"));
}

#[test]
fn blast_radius_is_deterministic_across_ten_trials() {
    let sources = read_fixture_sources();
    let changeset = changed_lib_changeset(&sources);

    let mut baseline: Option<Vec<(String, i32)>> = None;
    for _ in 0..10 {
        let engine = build_engine();
        let out = engine.analyze_context(sources.clone(), &changeset);
        let mut shape: Vec<(String, i32)> = out
            .items
            .iter()
            .filter(|i| i.check_id == "Context.BlastRadius")
            .map(|i| (i.title.clone(), i.score))
            .collect();
        shape.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        match &baseline {
            None => baseline = Some(shape),
            Some(b) => assert_eq!(b, &shape, "non-deterministic ordering across trials"),
        }
    }
}
