//! CD-40 lever 5 gate: topology-complete parity tests for
//! `Design.DuplicateExportName`'s incrementally-maintained per-name
//! index (`DenIndex` in `cofferdam-checks/src/design/duplicate_export_name.rs`).
//!
//! `finalize()` used to re-group every export in the corpus into a
//! `BTreeMap<name, Vec<NamedExport>>` on every call. It now maintains
//! that grouping incrementally across `Engine::analyze_incremental`
//! calls and only re-evaluates names whose group changed since the
//! last call. Each case below is required before that could be
//! considered safe — the shrinking-group and primary-migration traps
//! in particular are silent-corruption risks a from-scratch rebuild
//! would never exercise.
//!
//! Every case compares `analyze_incremental` against a fresh
//! `analyze_with_sources` over the same resulting file set —
//! byte-identical output is the bar, matching `incremental_parity.rs`
//! and `incremental_graph_topology.rs`.

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
                "{}|{}|{}:{}|{:?}|{:?}|{}|{:?}",
                i.check_id,
                i.file.to_string_lossy().replace('\\', "/"),
                i.location.line(),
                i.location.column(),
                i.severity,
                i.priority,
                i.message,
                i.related
                    .iter()
                    .map(|r| r.file.to_string_lossy().replace('\\', "/"))
                    .collect::<Vec<_>>()
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

fn den_issues(issues: &[cofferdam_core::Issue]) -> Vec<&cofferdam_core::Issue> {
    issues
        .iter()
        .filter(|i| i.check_id == "Design.DuplicateExportName")
        .collect()
}

/// Scenario A1: editing a file to drop the name it shared with
/// another file must make the finding disappear (group shrinks from
/// 2 occurrences to 1) — the "shrinking-group" trap. A cache that
/// only writes on emit, never clears on non-emit, would leave a
/// stale finding here.
#[test]
fn dropping_a_shared_export_name_clears_the_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let a = write(root, "a.ts", "export const shared = 1;\n");
    let b = write(root, "b.ts", "export const shared = 2;\n");

    let eng = engine();
    let mut state = AnalysisState::new();
    let sources = vec![
        (a.clone(), std::fs::read_to_string(&a).unwrap()),
        (b.clone(), std::fs::read_to_string(&b).unwrap()),
    ];
    let seeded = eng.analyze_incremental(&mut state, &sources, &[]);
    assert_eq!(
        den_issues(&seeded).len(),
        1,
        "both files export `shared` — expected one collision finding"
    );

    let edited_a_text = "export const notShared = 1;\n".to_string();
    let incremental =
        eng.analyze_incremental(&mut state, &[(a.clone(), edited_a_text.clone())], &[]);
    assert!(
        den_issues(&incremental).is_empty(),
        "renaming A's export away from `shared` must clear the collision: {:?}",
        den_issues(&incremental)
    );

    let reference = vec![(a, edited_a_text), (b, sources[1].1.clone())];
    assert_parity(&incremental, reference);
}

/// Scenario A2: the mirror image of A1 — editing a file to newly
/// collide with an existing name must produce a finding.
#[test]
fn adding_a_colliding_export_name_produces_a_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let a = write(root, "a.ts", "export const onlyA = 1;\n");
    let b = write(root, "b.ts", "export const shared = 2;\n");

    let eng = engine();
    let mut state = AnalysisState::new();
    let sources = vec![
        (a.clone(), std::fs::read_to_string(&a).unwrap()),
        (b.clone(), std::fs::read_to_string(&b).unwrap()),
    ];
    let seeded = eng.analyze_incremental(&mut state, &sources, &[]);
    assert!(den_issues(&seeded).is_empty());

    let edited_a_text = "export const shared = 1;\n".to_string();
    let incremental =
        eng.analyze_incremental(&mut state, &[(a.clone(), edited_a_text.clone())], &[]);
    assert_eq!(
        den_issues(&incremental).len(),
        1,
        "A now also exports `shared` — expected a new collision finding"
    );

    let reference = vec![(a, edited_a_text), (b, sources[1].1.clone())];
    assert_parity(&incremental, reference);
}

/// Scenario A4: three files export the same name; removing the
/// alphabetically-first (primary) occurrence must migrate the
/// finding's primary location and shrink `related`, not leave a
/// stale cached issue pointing at a removed file.
#[test]
fn removing_the_primary_occurrence_migrates_the_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let a = write(root, "a.ts", "export const shared = 1;\n");
    let b = write(root, "b.ts", "export const shared = 2;\n");
    let c = write(root, "c.ts", "export const shared = 3;\n");

    let eng = engine();
    let mut state = AnalysisState::new();
    let a_text = std::fs::read_to_string(&a).unwrap();
    let b_text = std::fs::read_to_string(&b).unwrap();
    let c_text = std::fs::read_to_string(&c).unwrap();
    let sources = vec![
        (a.clone(), a_text.clone()),
        (b.clone(), b_text.clone()),
        (c.clone(), c_text.clone()),
    ];
    let seeded = eng.analyze_incremental(&mut state, &sources, &[]);
    let seeded_den = den_issues(&seeded);
    assert_eq!(seeded_den.len(), 1);
    assert!(
        seeded_den[0]
            .file
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("a.ts"),
        "primary should be the alphabetically-first file: {:?}",
        seeded_den[0].file
    );
    assert_eq!(seeded_den[0].related.len(), 2);

    // Remove A (the primary). B becomes primary, related shrinks to
    // just C, and the message's file-count text updates.
    let incremental = eng.analyze_incremental(&mut state, &[], std::slice::from_ref(&a));
    let inc_den = den_issues(&incremental);
    assert_eq!(inc_den.len(), 1);
    assert!(
        inc_den[0]
            .file
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("b.ts"),
        "primary must migrate to B after A is removed: {:?}",
        inc_den[0].file
    );
    assert_eq!(inc_den[0].related.len(), 1);

    let reference = vec![(b, b_text), (c, c_text)];
    assert_parity(&incremental, reference);
}

/// Scenario A6: editing a file whose exports never collide with
/// anything must leave an existing, unrelated collision finding
/// byte-identical (the cache-reuse / no-op path).
#[test]
fn editing_an_unrelated_file_leaves_the_finding_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let a = write(root, "a.ts", "export const shared = 1;\n");
    let b = write(root, "b.ts", "export const shared = 2;\n");
    let unrelated = write(root, "u.ts", "export const soleUse = 1;\n");

    let eng = engine();
    let mut state = AnalysisState::new();
    let a_text = std::fs::read_to_string(&a).unwrap();
    let b_text = std::fs::read_to_string(&b).unwrap();
    let sources = vec![
        (a.clone(), a_text.clone()),
        (b.clone(), b_text.clone()),
        (
            unrelated.clone(),
            std::fs::read_to_string(&unrelated).unwrap(),
        ),
    ];
    let _ = eng.analyze_incremental(&mut state, &sources, &[]);

    let edited_unrelated_text = "export const soleUse = 2;\n".to_string();
    let incremental = eng.analyze_incremental(
        &mut state,
        &[(unrelated.clone(), edited_unrelated_text.clone())],
        &[],
    );
    assert_eq!(
        den_issues(&incremental).len(),
        1,
        "unrelated edit must not disturb the A/B collision"
    );

    let reference = vec![(a, a_text), (b, b_text), (unrelated, edited_unrelated_text)];
    assert_parity(&incremental, reference);
}

/// Scenario A7 (adapted): a third file joining an existing two-way
/// collision must widen the finding to three occurrences — the
/// dirty-set update correctly reacts to a brand-new file, not just
/// an edit to a file the index already knew about.
#[test]
fn a_third_file_joining_an_existing_collision_widens_the_finding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let client = write(root, "client/schema.ts", "export const UserId = 1;\n");
    let server = write(root, "server/schema.ts", "export const UserId = 2;\n");

    // These low-level engine tests don't configure per-check options
    // (no `exempt_boundary_pairs`), so this exercises the ordinary
    // third-file-un-exempts-nothing path — proving the dirty-set
    // update when a third contributor joins an existing group.
    let eng = engine();
    let mut state = AnalysisState::new();
    let client_text = std::fs::read_to_string(&client).unwrap();
    let server_text = std::fs::read_to_string(&server).unwrap();
    let sources = vec![
        (client.clone(), client_text.clone()),
        (server.clone(), server_text.clone()),
    ];
    let seeded = eng.analyze_incremental(&mut state, &sources, &[]);
    assert_eq!(den_issues(&seeded).len(), 1);

    let stray = write(root, "utils/misc.ts", "export const UserId = 3;\n");
    let stray_text = std::fs::read_to_string(&stray).unwrap();
    let incremental =
        eng.analyze_incremental(&mut state, &[(stray.clone(), stray_text.clone())], &[]);
    let inc_den = den_issues(&incremental);
    assert_eq!(inc_den.len(), 1, "still one collision, now three-way");
    assert_eq!(inc_den[0].related.len(), 2);

    let reference = vec![
        (client, client_text),
        (server, server_text),
        (stray, stray_text),
    ];
    assert_parity(&incremental, reference);
}

/// Scenario D24 (cross-cutting, DuplicateExportName-scoped): a longer
/// edit/remove/re-add/edit sequence against a single `AnalysisState`.
/// Index leaks from an earlier step often only surface a few steps
/// later, once a previously-touched name is touched again.
#[test]
fn a_sequence_of_edits_and_removals_stays_in_parity_at_every_step() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let a = write(root, "a.ts", "export const shared = 1;\n");
    let b = write(root, "b.ts", "export const shared = 2;\n");
    let c = write(root, "c.ts", "export const other = 1;\n");

    let eng = engine();
    let mut state = AnalysisState::new();
    let a_text = std::fs::read_to_string(&a).unwrap();
    let b_text = std::fs::read_to_string(&b).unwrap();
    let c_text = std::fs::read_to_string(&c).unwrap();

    // Step 1: seed all three.
    let step1 = eng.analyze_incremental(
        &mut state,
        &[
            (a.clone(), a_text.clone()),
            (b.clone(), b_text.clone()),
            (c.clone(), c_text.clone()),
        ],
        &[],
    );
    assert_parity(
        &step1,
        vec![
            (a.clone(), a_text.clone()),
            (b.clone(), b_text.clone()),
            (c.clone(), c_text.clone()),
        ],
    );

    // Step 2: remove B — collision disappears.
    let step2 = eng.analyze_incremental(&mut state, &[], std::slice::from_ref(&b));
    assert!(den_issues(&step2).is_empty());
    assert_parity(
        &step2,
        vec![(a.clone(), a_text.clone()), (c.clone(), c_text.clone())],
    );

    // Step 3: re-add B under a name that now collides with C instead.
    let b_text_v2 = "export const other = 2;\n".to_string();
    let step3 = eng.analyze_incremental(&mut state, &[(b.clone(), b_text_v2.clone())], &[]);
    assert_eq!(den_issues(&step3).len(), 1);
    assert_parity(
        &step3,
        vec![
            (a.clone(), a_text.clone()),
            (b.clone(), b_text_v2.clone()),
            (c.clone(), c_text.clone()),
        ],
    );

    // Step 4: edit A (unrelated to the B/C collision) — collision
    // must survive untouched, and A's own export must still read
    // correctly if it collides with nothing.
    let a_text_v2 = "export const soleA = 1;\n".to_string();
    let step4 = eng.analyze_incremental(&mut state, &[(a.clone(), a_text_v2.clone())], &[]);
    assert_eq!(den_issues(&step4).len(), 1);
    assert_parity(&step4, vec![(a, a_text_v2), (b, b_text_v2), (c, c_text)]);
}
