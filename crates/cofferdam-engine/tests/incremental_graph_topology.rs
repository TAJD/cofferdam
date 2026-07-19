//! CD-40 lever 3 gate: topology-complete parity tests for the
//! incrementally-maintained `CANONICAL_GRAPH` corpus slot.
//!
//! `Engine::finalize_and_filter`'s `GraphUpdate::Incremental` branch
//! mutates the persisted graph in place (`Graph::remove_file` +
//! `apply_records`) instead of rebuilding it from scratch every call.
//! `Design.OrphanExport` is the only built-in check reading
//! `CANONICAL_GRAPH` today, so it's the load-bearing consumer these
//! tests exercise. Each case is required by CD-40's ticket body before
//! lever 3 could be considered safe:
//!
//! - edit a file that imports an unchanged file (shared-node case)
//! - remove a file that another, untouched file also imports (the
//!   "trap": `Graph::remove_file` must decrement the shared node's
//!   owner set rather than deleting it outright)
//! - add a file that closes an import cycle
//! - re-add a previously-removed importer
//!
//! Every case compares `analyze_incremental` against a fresh
//! `analyze_with_sources` over the same resulting file set —
//! byte-identical output is the bar, matching `incremental_parity.rs`.

use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{AnalysisState, Engine};

fn engine() -> Engine {
    Engine::new(all_builtins())
}

fn write(root: &Path, rel: &str, content: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir -p");
    std::fs::write(&path, content).expect("write fixture file");
    path
}

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

fn assert_parity(incremental: &[cofferdam_core::Issue], reference_sources: Vec<(PathBuf, String)>) {
    let (from_scratch, _) = engine().analyze_with_sources(reference_sources);
    assert_eq!(
        full_identity(&from_scratch),
        full_identity(incremental),
        "incremental result must match a from-scratch analyze over the same file set"
    );
}

fn orphan_names(issues: &[cofferdam_core::Issue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.check_id == "Design.OrphanExport")
        .map(|i| i.message.clone())
        .collect()
}

/// Edit A (which imports unchanged B) — B is a shared node A doesn't
/// solely own; re-editing A must not disturb B's contribution.
#[test]
fn editing_an_importer_leaves_unchanged_shared_target_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let b = write(
        root,
        "b.ts",
        "export function b(): number {\n  return 1;\n}\n",
    );
    let a = write(
        root,
        "a.ts",
        "import { b } from \"./b\";\nexport function a(): number {\n  return b();\n}\n",
    );

    let eng = engine();
    let mut state = AnalysisState::new();
    let sources = vec![
        (a.clone(), std::fs::read_to_string(&a).unwrap()),
        (b.clone(), std::fs::read_to_string(&b).unwrap()),
    ];
    let _ = eng.analyze_incremental(&mut state, &sources, &[]);

    // Edit A (still imports b, adds an unrelated statement) — b must
    // still read as consumed, not orphaned.
    let edited_a_text = format!("{}\nconst __marker = 1;\n", sources[0].1);
    let incremental =
        eng.analyze_incremental(&mut state, &[(a.clone(), edited_a_text.clone())], &[]);

    let orphans = orphan_names(&incremental);
    assert!(
        !orphans.iter().any(|m| m.contains("`b`")),
        "unchanged shared import target must not be flagged orphan after editing its importer: {orphans:?}"
    );

    let reference = vec![(a, edited_a_text), (b, sources[1].1.clone())];
    assert_parity(&incremental, reference);
}

/// Remove B, which is imported by BOTH A and (untouched) C — the
/// "trap" scenario `Graph::remove_file`'s doc comment warns about.
/// Removing A's contribution to B's shared node must decrement its
/// owner set, not delete the node outright while C still owns it.
#[test]
fn removing_one_importer_of_a_shared_target_keeps_other_importers_correct() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let shared = write(
        root,
        "shared.ts",
        "export function shared(): number {\n  return 1;\n}\n",
    );
    let a = write(
        root,
        "a.ts",
        "import { shared } from \"./shared\";\nexport function a(): number {\n  return shared();\n}\n",
    );
    let c = write(
        root,
        "c.ts",
        "import { shared } from \"./shared\";\nexport function c(): number {\n  return shared();\n}\n",
    );

    let eng = engine();
    let mut state = AnalysisState::new();
    let a_text = std::fs::read_to_string(&a).unwrap();
    let c_text = std::fs::read_to_string(&c).unwrap();
    let shared_text = std::fs::read_to_string(&shared).unwrap();
    let sources = vec![
        (a.clone(), a_text.clone()),
        (c.clone(), c_text.clone()),
        (shared.clone(), shared_text.clone()),
    ];
    let _ = eng.analyze_incremental(&mut state, &sources, &[]);

    // Remove A. C still imports `shared` — `shared`'s export must
    // still read as consumed via C, not suddenly orphaned because A
    // (one of two owners of `shared`'s file node) dropped out.
    let incremental = eng.analyze_incremental(&mut state, &[], std::slice::from_ref(&a));

    let orphans = orphan_names(&incremental);
    assert!(
        !orphans.iter().any(|m| m.contains("`shared`")),
        "shared export still consumed by an untouched importer must not be flagged orphan \
         after a DIFFERENT importer is removed: {orphans:?}"
    );

    let reference = vec![(c, c_text), (shared, shared_text)];
    assert_parity(&incremental, reference);
}

/// Adding a file that closes an import cycle (A -> B -> A) must be
/// visible to `Design.ImportCycle` exactly as a from-scratch analyze
/// would report it.
#[test]
fn adding_a_file_that_closes_an_import_cycle_matches_from_scratch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // B imports A from the start, but A doesn't import B yet — no
    // cycle. Then a second incremental call edits A to import B,
    // closing the cycle.
    let b = write(
        root,
        "b.ts",
        "import { a } from \"./a\";\nexport function b(): number {\n  return a();\n}\n",
    );
    let a_v1 = "export function a(): number {\n  return 1;\n}\n";
    let a = write(root, "a.ts", a_v1);

    let eng = engine();
    let mut state = AnalysisState::new();
    let b_text = std::fs::read_to_string(&b).unwrap();
    let _ = eng.analyze_incremental(
        &mut state,
        &[(a.clone(), a_v1.to_string()), (b.clone(), b_text.clone())],
        &[],
    );

    let a_v2 = "import { b } from \"./b\";\nexport function a(): number {\n  return b();\n}\n";
    let incremental = eng.analyze_incremental(&mut state, &[(a.clone(), a_v2.to_string())], &[]);

    let reference = vec![(a, a_v2.to_string()), (b, b_text)];
    assert_parity(&incremental, reference);
}

/// Re-add a previously-removed importer: remove A (an importer of
/// B), then add a file at A's old path back — B's node/ownership must
/// recover exactly as a from-scratch analyze of the final set would
/// produce, not leak stale ownership state from before the removal.
#[test]
fn re_adding_a_previously_removed_importer_matches_from_scratch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let b = write(
        root,
        "b.ts",
        "export function b(): number {\n  return 1;\n}\n",
    );
    let a_text =
        "import { b } from \"./b\";\nexport function a(): number {\n  return b();\n}\n".to_string();
    let a = write(root, "a.ts", &a_text);

    let eng = engine();
    let mut state = AnalysisState::new();
    let b_text = std::fs::read_to_string(&b).unwrap();
    let _ = eng.analyze_incremental(
        &mut state,
        &[(a.clone(), a_text.clone()), (b.clone(), b_text.clone())],
        &[],
    );

    // Remove A — b becomes orphaned (nobody imports it any more).
    let _ = eng.analyze_incremental(&mut state, &[], std::slice::from_ref(&a));

    // Re-add A with the same import — b should read as consumed
    // again, identical to a from-scratch analyze of {a, b}.
    let incremental = eng.analyze_incremental(&mut state, &[(a.clone(), a_text.clone())], &[]);

    let orphans = orphan_names(&incremental);
    assert!(
        !orphans.iter().any(|m| m.contains("`b`")),
        "b must read as consumed again once its importer is re-added: {orphans:?}"
    );

    let reference = vec![(a, a_text), (b, b_text)];
    assert_parity(&incremental, reference);
}
