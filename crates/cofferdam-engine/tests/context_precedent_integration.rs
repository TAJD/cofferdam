//! Fixture-repo integration test for `Context.Precedent` (CD-161).
//!
//! Reads `examples/context_precedent/**` from disk — a real directory
//! fixture, not inline strings — and runs the full engine (all context
//! providers) against it via `Engine::analyze_context`, mirroring how
//! `cofferdam context` itself drives the provider (CLI wiring lives in
//! `cofferdam-cli/src/context_cmd.rs`, out of scope for a unit test).

use cofferdam_core::ChangeSet;
use cofferdam_engine::Engine;
use std::path::{Path, PathBuf};

/// `examples/` at the repo root, two levels up from this crate's
/// `Cargo.toml` (`crates/cofferdam-engine/` -> workspace root),
/// mirroring the fixture-path derivation used elsewhere in this test
/// suite (see `engine_integration.rs`).
fn fixture_dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("repo root")
        .join("examples")
        .join("context_precedent")
        .join(sub)
}

/// Reads every `*.ts` file directly inside `dir` (non-recursive — the
/// fixture has no nested subdirectories) as `(path, source)` pairs.
fn read_ts_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("ts") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            out.push((path, text));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn established_pattern_surfaces_sibling_exemplars_for_a_new_file() {
    let handlers = fixture_dir("handlers");
    if !handlers.exists() {
        // Shouldn't happen in CI — fixture is checked in — but mirrors
        // the defensive skip other fixture-backed tests in this suite
        // use for a missing example.
        return;
    }
    let sources = read_ts_files(&handlers);
    let new_file = handlers.join("delete_user.ts");
    assert!(
        sources.iter().any(|(p, _)| p == &new_file),
        "fixture must include delete_user.ts"
    );

    let engine = Engine::new(cofferdam_checks::all_context_providers());
    let changeset = ChangeSet::from_files([new_file.clone()]);
    let out = engine.analyze_context(sources, &changeset);

    let precedent: Vec<_> = out
        .items
        .iter()
        .filter(|i| i.check_id == "Context.Precedent")
        .collect();
    assert_eq!(
        precedent.len(),
        1,
        "expected one Context.Precedent item for the new file; got {:?}",
        out.items
    );
    let item = precedent[0];
    assert!(!item.pinned, "Context.Precedent items must never be pinned");
    assert!(item.body.contains("create_user.ts"));
    assert!(item.body.contains("CreateUserRequest"));
    assert!(
        !item.body.contains("delete_user.ts"),
        "the changed file itself must not appear as an exemplar"
    );
}

#[test]
fn unrelated_edit_produces_no_precedent_items() {
    let unrelated = fixture_dir("unrelated");
    if !unrelated.exists() {
        return;
    }
    let sources = read_ts_files(&unrelated);
    let changed = unrelated.join("format.ts");
    assert!(sources.iter().any(|(p, _)| p == &changed));

    let engine = Engine::new(cofferdam_checks::all_context_providers());
    let changeset = ChangeSet::from_files([changed]);
    let out = engine.analyze_context(sources, &changeset);

    assert!(
        out.items.iter().all(|i| i.check_id != "Context.Precedent"),
        "a lone file with no siblings must never surface a precedent item; got {:?}",
        out.items
    );
}
