//! CD-40 lever 4: a consistency check that reads another file's
//! pass-1 evidence in `pass2` (i.e. `pass2_is_file_local() == false`,
//! the default) MUST still be recomputed for every currently-known
//! file on every `analyze_incremental` call — not just `changed`
//! files — since an edit to one file can change what a project-wide
//! aggregate check emits for an untouched file.
//!
//! `incremental_parity.rs`'s fixture-driven tests don't exercise this
//! path: the only real consistency check registered today
//! (`Consistency.QuoteStyle`) is genuinely file-local, so the
//! `cacheable == false` fallback branch added by lever 4 has no other
//! coverage. This test supplies a purpose-built cross-file consistency
//! check to pin that fallback.

use std::collections::HashSet;
use std::path::PathBuf;

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusIndex, CorpusKey, Issue, Location, Priority,
    Severity, SourceFile, Span,
};
use cofferdam_engine::{AnalysisState, Engine};

/// Corpus slot: every file whose text currently contains the literal
/// "TRIGGER" marker.
static TRIGGERED: CorpusKey<HashSet<PathBuf>> = CorpusKey::new("Test.CrossFileTrigger.triggered");

/// Pass 1 records whether this file contains "TRIGGER"; pass 2 emits
/// one issue per file for as long as ANY file in the project does —
/// a genuinely project-wide aggregate, unlike `QuoteStyle`'s per-file
/// verdict.
struct CrossFileTrigger;

const META: CheckMeta = CheckMeta {
    id: "Test.CrossFileTrigger",
    category: Category::Consistency,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "test-only cross-file consistency check",
    body: "test-only cross-file consistency check",
    requires_types: false,
    consistency: true,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for CrossFileTrigger {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &CorpusIndex) {
        corpus.register_removable(&TRIGGERED, |slot, path| {
            slot.remove(path);
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        if file.text.contains("TRIGGER") {
            ctx.corpus
                .with_slot(&TRIGGERED, |slot| slot.insert(file.path.clone()));
        }
        Vec::new()
    }

    // Deliberately does NOT override `pass2_is_file_local` — the
    // default `false` is exactly what this test pins.
    fn pass2(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let any_triggered = ctx.corpus.with_slot(&TRIGGERED, |slot| !slot.is_empty());
        if !any_triggered {
            return Vec::new();
        }
        vec![Issue {
            check_id: META.id.to_string(),
            message: "project has a triggered file".into(),
            file: file.path.clone(),
            location: Location::from_span(
                &file.path,
                Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
            ),
            priority: Priority(META.base_priority),
            severity: Severity::Info,
            related: Vec::new(),
        }]
    }
}

fn full_identity(issues: &[Issue]) -> Vec<String> {
    let mut ids: Vec<String> = issues
        .iter()
        .map(|i| {
            format!(
                "{}|{}",
                i.check_id,
                i.file.to_string_lossy().replace('\\', "/")
            )
        })
        .collect();
    ids.sort();
    ids
}

#[test]
fn incremental_recomputes_cross_file_pass2_for_unchanged_files() {
    let engine = Engine::new(vec![Box::new(CrossFileTrigger)]);
    let a = PathBuf::from("/virtual/a.ts");
    let b = PathBuf::from("/virtual/b.ts");

    let mut state = AnalysisState::new();
    let seeded = engine.analyze_incremental(
        &mut state,
        &[
            (a.clone(), "const x = 1;\n".to_string()),
            (b.clone(), "const y = 2;\n".to_string()),
        ],
        &[],
    );
    assert!(
        seeded.is_empty(),
        "neither file contains TRIGGER yet, expected no issues"
    );

    // Edit only `a` to contain the marker. `b` is untouched.
    let edited = engine.analyze_incremental(
        &mut state,
        &[(a.clone(), "const x = 1; // TRIGGER\n".to_string())],
        &[],
    );

    let (from_scratch, _) = engine.analyze_with_sources(vec![
        (a.clone(), "const x = 1; // TRIGGER\n".to_string()),
        (b.clone(), "const y = 2;\n".to_string()),
    ]);

    assert_eq!(
        full_identity(&from_scratch),
        full_identity(&edited),
        "editing `a` must also update `b`'s (unchanged) pass-2 output, matching a \
         from-scratch analysis — `b`'s cached pass-2 issues from the seed call must \
         NOT be reused for a check with pass2_is_file_local() == false"
    );
    assert_eq!(
        edited.len(),
        2,
        "expected both a.ts and b.ts to carry the cross-file finding: {edited:?}"
    );
}
