//! Integration: incremental analysis matches a from-scratch run,
//! byte-identical (CD-32).
//!
//! `Engine::analyze_incremental` is only useful if a single-file edit
//! produces EXACTLY the findings a full `analyze_with_sources` over
//! the same resulting file set would — any drift (a stale corpus
//! contribution left behind, a missed re-aggregation) would silently
//! corrupt `cofferdam watch`'s output. These tests hold one
//! `AnalysisState` across several incremental calls — mutate one
//! fixture, add one, remove one — and compare each result against a
//! fresh from-scratch analyze of the same file set.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::{AnalysisState, Engine};

fn engine() -> Engine {
    Engine::new(all_builtins())
}

/// Every `.ts` fixture directly under `examples/` — same universe
/// `parallel_determinism.rs` sweeps, so this exercises per-file
/// checks, cross-file corpus checks, consistency/pass2 checks, and
/// the canonical-graph finalize pass across many files at once.
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
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources
}

/// Full identity of one issue, mirroring `parallel_determinism.rs` —
/// file, check, exact location, message, severity, priority — so a
/// mismatch anywhere fails loudly.
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
fn incremental_initial_population_matches_from_scratch() {
    let sources = example_sources();

    let from_scratch_engine = engine();
    let (from_scratch, _) = from_scratch_engine.analyze_with_sources(sources.clone());

    let incremental_engine = engine();
    let mut state = AnalysisState::new();
    let incremental = incremental_engine.analyze_incremental(&mut state, &sources, &[]);

    assert_eq!(
        full_identity(&from_scratch),
        full_identity(&incremental),
        "seeding an empty AnalysisState via analyze_incremental must match \
         a from-scratch analyze_with_sources over the same file set"
    );
}

#[test]
fn incremental_single_file_edit_matches_from_scratch() {
    let sources = example_sources();

    let incremental_engine = engine();
    let mut state = AnalysisState::new();
    let _ = incremental_engine.analyze_incremental(&mut state, &sources, &[]);

    // Mutate exactly one fixture — append a line that trips a simple,
    // deterministic check (unused import) so the edit actually shifts
    // findings, not just re-runs the same output.
    let (mutated_path, original_text) = sources.first().expect("at least one fixture").clone();
    let mutated_text = format!("{original_text}\nconst __cd32_incremental_marker = 1;\n");

    let incremental = incremental_engine.analyze_incremental(
        &mut state,
        &[(mutated_path.clone(), mutated_text.clone())],
        &[],
    );

    // From-scratch reference: the full fixture set with just that one
    // file's text replaced.
    let mut reference_sources = sources;
    reference_sources[0] = (mutated_path, mutated_text);
    let (from_scratch, _) = engine().analyze_with_sources(reference_sources);

    assert_eq!(
        full_identity(&from_scratch),
        full_identity(&incremental),
        "analyze_incremental after a single-file edit must match a from-scratch \
         analyze_with_sources over the resulting file set, byte-identical"
    );
}

#[test]
fn incremental_file_removal_matches_from_scratch() {
    let sources = example_sources();

    let incremental_engine = engine();
    let mut state = AnalysisState::new();
    let _ = incremental_engine.analyze_incremental(&mut state, &sources, &[]);

    let (removed_path, _) = sources.first().expect("at least one fixture").clone();
    let incremental = incremental_engine.analyze_incremental(
        &mut state,
        &[],
        std::slice::from_ref(&removed_path),
    );

    let reference_sources: Vec<_> = sources
        .into_iter()
        .filter(|(p, _)| p != &removed_path)
        .collect();
    let (from_scratch, _) = engine().analyze_with_sources(reference_sources);

    assert_eq!(
        full_identity(&from_scratch),
        full_identity(&incremental),
        "analyze_incremental after removing a file must match a from-scratch \
         analyze_with_sources over the remaining file set, byte-identical"
    );
}

#[test]
fn incremental_file_addition_matches_from_scratch() {
    let sources = example_sources();

    // Seed state with all but one fixture, then add it back via a
    // second incremental call.
    let (added_path, added_text) = sources.last().expect("at least one fixture").clone();
    let seed_sources: Vec<_> = sources
        .iter()
        .filter(|(p, _)| p != &added_path)
        .cloned()
        .collect();

    let incremental_engine = engine();
    let mut state = AnalysisState::new();
    let _ = incremental_engine.analyze_incremental(&mut state, &seed_sources, &[]);

    let incremental =
        incremental_engine.analyze_incremental(&mut state, &[(added_path, added_text)], &[]);

    let (from_scratch, _) = engine().analyze_with_sources(sources);

    assert_eq!(
        full_identity(&from_scratch),
        full_identity(&incremental),
        "analyze_incremental after adding a file back must match a from-scratch \
         analyze_with_sources over the full file set, byte-identical"
    );
}
