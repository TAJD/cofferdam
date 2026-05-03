//! Property-based tests for `cofferdam-engine::Engine` idempotency.
//!
//! **Invariant**: running `Engine::analyze` twice over the same set of files
//! produces identical `Vec<Issue>` results — same length and, for each pair of
//! issues at the same index, identical field values.
//!
//! This guards against any non-determinism introduced by: uninitialized HashMap
//! iteration order leaking into the sort, corpus-slot state accumulated across
//! calls (if the engine were ever reused), or file-system walker non-determinism
//! propagating past the sort in `analyze_with_text`.
//!
//! Cases are kept low (32) because each case involves disk I/O and a full oxc
//! parse pass; this keeps the suite fast enough for regular `cargo test` runs.

use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;

use cofferdam_checks::all_builtins;
use cofferdam_core::Issue;
use cofferdam_engine::Engine;

// ──────────────────────────────────────────────────────────────────────────
// Strategies
// ──────────────────────────────────────────────────────────────────────────

/// A small, syntactically valid TypeScript snippet.
///
/// We keep snippets tiny so oxc parses them without errors — parse errors are
/// still deterministic, but they're boring from an idempotency perspective.
fn ts_snippet() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("const x: number = 1;"),
        Just("export function f(a: string): string { return a; }"),
        Just("const arr = [1, 2, 3].map(n => n * 2);"),
        Just("interface Foo { bar: string; baz: number; }"),
        Just("type Maybe<T> = T | null | undefined;"),
        Just("export default class Counter { count = 0; inc() { this.count++; } }"),
        Just("const obj = { a: 1, b: 2, c: 3 };"),
    ]
}

/// File stem: 1–6 alphanumeric chars.
fn file_stem() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,5}".prop_map(|s| s)
}

/// One of the two most common TS extensions.
fn ts_ext() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("ts"), Just("tsx"),]
}

/// A single `(filename, content)` pair.
fn ts_file() -> impl Strategy<Value = (String, &'static str)> {
    (file_stem(), ts_ext(), ts_snippet())
        .prop_map(|(stem, ext, body)| (format!("{}.{}", stem, ext), body))
}

/// A list of 1–5 distinct-named TS files to write into the temp dir.
fn file_list() -> impl Strategy<Value = Vec<(String, &'static str)>> {
    prop::collection::vec(ts_file(), 1..=5).prop_map(|mut files| {
        // Deduplicate by name so we don't collide on disk.
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files.dedup_by(|a, b| a.0 == b.0);
        files
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────

/// A stable string key for an Issue used in proptest failure messages.
///
/// We project the fields we care about into a string so that failure output is
/// human-readable. We then also compare the full vector length to catch any
/// extra/missing issues.
fn issue_key(i: &Issue) -> String {
    format!(
        "{}|{}|{}:{}:{}",
        i.check_id,
        i.file.display(),
        i.span.line,
        i.span.column,
        i.priority.0
    )
}

// ──────────────────────────────────────────────────────────────────────────
// Property test
// ──────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, .. ProptestConfig::default() })]

    /// Running `Engine::analyze` twice on the same paths yields identical issues.
    #[test]
    fn prop_engine_analyze_is_idempotent(files in file_list()) {
        let temp_dir = TempDir::new().expect("temp dir");

        let mut paths: Vec<PathBuf> = Vec::new();
        for (name, content) in &files {
            let p = temp_dir.path().join(name);
            fs::write(&p, content).expect("write file");
            paths.push(p);
        }
        // Sort paths so the order fed to the engine is deterministic across calls.
        paths.sort();

        let engine = Engine::new(all_builtins());

        let run1 = engine.analyze(&paths).expect("analyze run 1");
        let run2 = engine.analyze(&paths).expect("analyze run 2");

        prop_assert_eq!(
            run1.len(),
            run2.len(),
            "issue count differs: run1={} run2={} files={:?}",
            run1.len(),
            run2.len(),
            files.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );

        let keys1: Vec<String> = run1.iter().map(issue_key).collect();
        let keys2: Vec<String> = run2.iter().map(issue_key).collect();
        prop_assert_eq!(
            &keys1,
            &keys2,
            "issue keys differ: run1={:?} run2={:?}",
            keys1,
            keys2
        );
    }
}
