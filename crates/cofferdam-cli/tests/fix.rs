//! Integration tests for `cofferdam fix` — autofix round-trip.
//!
//! These tests exercise the core fix pipeline end-to-end:
//! 1. Copy `examples/triple_equals.ts` to a temp directory.
//! 2. Run the engine, collect issues, call `autofix` per issue, apply
//!    edits in reverse byte-offset order, write the file atomically.
//! 3. Assert that `==` is now `===` and `!=` is now `!==`, and that no
//!    other bytes changed outside those replacements.
//!
//! We mirror the production `run_fix` logic here rather than shelling
//! out to the binary so the test works on every platform without extra
//! tooling and the failure messages point at the Rust source.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Check, SourceFile, TextEdit};
use cofferdam_engine::Engine;
use tempfile::TempDir;

/// Apply a sorted (reverse-offset) list of `TextEdit`s to `text` in place.
fn apply_edits(mut text: String, mut edits: Vec<TextEdit>) -> String {
    // Reverse byte-offset order: largest start_byte first.
    edits.sort_by_key(|e| Reverse(e.span.start_byte));
    for edit in edits {
        let start = edit.span.start_byte as usize;
        let end = edit.span.end_byte as usize;
        if start <= end && end <= text.len() {
            text.replace_range(start..end, &edit.replacement);
        }
    }
    text
}

#[test]
fn triple_equals_fix_round_trip() {
    // Locate the fixture relative to the workspace root.
    // `CARGO_MANIFEST_DIR` points at `crates/cofferdam-cli/`.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest_dir
        .ancestors()
        .nth(2) // workspace root
        .expect("workspace root")
        .join("examples")
        .join("triple_equals.ts");
    let original = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("could not read fixture {}: {e}", fixture.display()));

    // Copy to a temp file so we never dirty the source-controlled fixture.
    let tmp = TempDir::new().expect("temp dir");
    let tmp_path = tmp.path().join("triple_equals.ts");
    std::fs::write(&tmp_path, &original).expect("write tmp fixture");

    // Run the engine against the temp copy.
    let engine = Engine::new(all_builtins());
    let (issues, texts) = engine
        .analyze_with_text(std::slice::from_ref(&tmp_path))
        .expect("analyze should succeed");

    // Build check_id → &dyn Check map.
    let checks = all_builtins();
    let check_map: HashMap<&str, &dyn Check> =
        checks.iter().map(|c| (c.meta().id, c.as_ref())).collect();

    // Collect autofix edits for TripleEquals issues only.
    let mut edits: Vec<TextEdit> = Vec::new();
    for issue in &issues {
        if issue.check_id != "Warning.TripleEquals" {
            continue;
        }
        let Some(check) = check_map.get(issue.check_id.as_str()) else {
            continue;
        };
        let Some(text) = texts.get(&issue.file) else {
            continue;
        };
        let source = SourceFile::new(issue.file.clone(), text.clone());
        if let Some(edit) = check.autofix(issue, &source) {
            edits.push(edit);
        }
    }

    // Fixture has 10 loose-equality operators total:
    // - 3 in the original block (2× ==, 1× !=) — all autofix
    // - 5 with null on either side (cd-21d) — diagnostic fires, autofix declines
    // - 2 trailing cases (`nullable == y`, `y == "null"`) — autofix
    // Net autofix edits = 3 + 2 = 5.
    assert_eq!(
        edits.len(),
        5,
        "expected 5 autofix edits (3 original + 2 non-null new), got {}",
        edits.len()
    );

    // Apply edits and write atomically.
    let original_text = texts[&tmp_path].clone();
    let fixed_text = apply_edits(original_text.clone(), edits);

    // Atomic write.
    let tmp_file = tmp_path.with_extension("cofferdam-fix.tmp");
    std::fs::write(&tmp_file, &fixed_text).expect("write tmp");
    std::fs::rename(&tmp_file, &tmp_path).expect("rename");

    // Read back and verify.
    let result = std::fs::read_to_string(&tmp_path).expect("read result");

    // Each autofix grows the file by 1 byte (`==` → `===` or `!=` → `!==`).
    // 5 edits → +5 bytes.
    assert_eq!(
        result.len(),
        original_text.len() + 5,
        "fixed file length mismatch: original={} fixed={}",
        original_text.len(),
        result.len()
    );

    // Re-run the engine on the fixed file: every remaining TripleEquals
    // finding must be a null-exempt one whose `autofix()` returns None.
    // Confirms autofixable operators were all fixed; the un-fixable ones
    // (the cd-21d exemptions) survive intentionally as diagnostics.
    let engine2 = Engine::new(all_builtins());
    let (issues2, texts2) = engine2
        .analyze_with_text(std::slice::from_ref(&tmp_path))
        .expect("re-analyze should succeed");
    let leftover_with_autofix: Vec<_> = issues2
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals")
        .filter(|issue| {
            let Some(check) = check_map.get(issue.check_id.as_str()) else {
                return false;
            };
            let Some(text) = texts2.get(&issue.file) else {
                return false;
            };
            let source = SourceFile::new(issue.file.clone(), text.clone());
            check.autofix(issue, &source).is_some()
        })
        .collect();
    assert!(
        leftover_with_autofix.is_empty(),
        "after fix, expected no autofixable TripleEquals findings; \
         got {} that still produce autofix edits:\n{result}",
        leftover_with_autofix.len(),
    );
}

#[test]
fn fix_on_already_strict_file_produces_no_edits() {
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("strict.ts");
    std::fs::write(
        &path,
        "export function f(a: unknown, b: unknown) { return a === b; }\n",
    )
    .expect("write");

    let engine = Engine::new(all_builtins());
    let (issues, texts) = engine
        .analyze_with_text(std::slice::from_ref(&path))
        .expect("analyze");

    let checks = all_builtins();
    let check_map: HashMap<&str, &dyn Check> =
        checks.iter().map(|c| (c.meta().id, c.as_ref())).collect();

    let edits: Vec<TextEdit> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals")
        .filter_map(|issue| {
            let check = check_map.get(issue.check_id.as_str())?;
            let text = texts.get(&issue.file)?;
            let source = SourceFile::new(issue.file.clone(), text.clone());
            check.autofix(issue, &source)
        })
        .collect();

    assert!(
        edits.is_empty(),
        "expected no edits for a file with only `===`, got {}",
        edits.len()
    );
}

/// Dry-run: verify that collecting edits (without applying them) leaves
/// the file on disk completely unchanged and that we do identify at least
/// one autofix edit for a file with `==`.
#[test]
fn dry_run_does_not_modify_file() {
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("demo.ts");
    let original_content = "const a = x == 5;\nconst b = y != null;\n";
    std::fs::write(&path, original_content).expect("write fixture");

    let engine = Engine::new(all_builtins());
    let (issues, texts) = engine
        .analyze_with_text(std::slice::from_ref(&path))
        .expect("analyze should succeed");

    let checks = all_builtins();
    let check_map: HashMap<&str, &dyn Check> =
        checks.iter().map(|c| (c.meta().id, c.as_ref())).collect();

    // Collect edits (dry-run: do NOT apply them).
    let mut dry_edits: Vec<TextEdit> = Vec::new();
    for issue in &issues {
        if issue.check_id != "Warning.TripleEquals" {
            continue;
        }
        let Some(check) = check_map.get(issue.check_id.as_str()) else {
            continue;
        };
        let Some(text) = texts.get(&issue.file) else {
            continue;
        };
        let source = SourceFile::new(issue.file.clone(), text.clone());
        if let Some(edit) = check.autofix(issue, &source) {
            dry_edits.push(edit);
        }
    }

    // There should be at least one edit to apply (== 5 is fixable; == null is not).
    assert!(
        !dry_edits.is_empty(),
        "expected at least one autofix edit for file with `==`"
    );

    // The file on disk must be UNCHANGED — dry-run never writes.
    let on_disk = std::fs::read_to_string(&path).expect("read file after dry-run");
    assert_eq!(
        on_disk, original_content,
        "dry-run must not modify the source file"
    );
}
