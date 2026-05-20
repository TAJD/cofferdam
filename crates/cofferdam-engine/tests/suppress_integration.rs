//! Integration tests for suppression directives (cd-5t7).

use tempfile::TempDir;

use cofferdam_checks::all_builtins;
use cofferdam_engine::Engine;

#[test]
fn suppress_next_line_all_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line\nif (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have no triple-equals issue on line 2 due to suppression
    let triple_equals_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(
        triple_equals_on_line_2.is_empty(),
        "expected no TripleEquals on line 2 (should be suppressed)"
    );
}

#[test]
fn suppress_next_line_specific_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line Warning.TripleEquals\nif (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have no TripleEquals on line 2
    let triple_equals_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(triple_equals_on_line_2.is_empty());
}

#[test]
fn suppress_next_line_skips_blanks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line\n\nif (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Blank line 2 should not suppress anything
    // Line 3 (first non-blank) should be suppressed
    let triple_equals_on_line_3: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 3)
        .collect();
    assert!(
        triple_equals_on_line_3.is_empty(),
        "expected no TripleEquals on line 3 (suppressed by line 1 directive)"
    );
}

#[test]
fn suppress_block_all_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "/* cofferdam-disable */\nif (a == b) { }\n/* cofferdam-enable */\nif (x == y) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Line 2 should be suppressed
    let triple_equals_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(
        triple_equals_on_line_2.is_empty(),
        "expected no TripleEquals on line 2 (inside disable block)"
    );

    // Line 4 should NOT be suppressed
    let triple_equals_on_line_4: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 4)
        .collect();
    assert!(
        !triple_equals_on_line_4.is_empty(),
        "expected TripleEquals on line 4 (outside disable block)"
    );
}

#[test]
fn suppress_block_specific_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code =
        "/* cofferdam-disable Warning.TripleEquals */\nif (a == b) { }\n/* cofferdam-enable */";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Line 2 should suppress TripleEquals but not other checks
    let triple_equals_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(
        triple_equals_on_line_2.is_empty(),
        "expected no TripleEquals on line 2"
    );
}

#[test]
fn suppress_block_no_matching_enable() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "/* cofferdam-disable Warning.TripleEquals */\nif (a == b) { }\nif (x == y) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Both lines 2 and 3 should be suppressed (extends to EOF)
    let triple_equals_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    let triple_equals_on_line_3: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 3)
        .collect();
    assert!(
        triple_equals_on_line_2.is_empty(),
        "line 2 should be suppressed"
    );
    assert!(
        triple_equals_on_line_3.is_empty(),
        "line 3 should be suppressed (block extends to EOF)"
    );
}

#[test]
fn suppress_directive_at_eof() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "const x = 1;\n// cofferdam-disable-next-line";
    std::fs::write(&file_path, code).expect("write file");

    // Should not panic; directive on last line is a no-op
    let _issues = engine.analyze(&[&file_path]).expect("analyze");
    // Just verify no crash
}

#[test]
fn without_suppression_triple_equals_fires() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "if (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have a TripleEquals issue on line 1
    let triple_equals: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals")
        .collect();
    assert!(
        !triple_equals.is_empty(),
        "expected at least one TripleEquals issue without suppression"
    );
}

// ============================================================
// cd-wqc regression: finalize-only emitters + UnusedSuppression
// ============================================================

/// A `cofferdam-ignore-file: Warning.UnusedImport` directive over a barrel
/// that genuinely re-exports an unconsumed symbol must NOT produce
/// `Consistency.UnusedSuppression`.  Before the two-phase finalize fix, the
/// snapshot was taken before `Warning.UnusedImport::finalize` ran, so the
/// directive appeared stale even though it was actively suppressing 1 finding.
#[test]
fn unused_import_suppress_file_directive_not_flagged_as_stale() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");

    // foo.ts — declares Foo; this is NOT a re-export, so UnusedImport won't
    // flag foo.ts itself.
    let foo_path = temp_dir.path().join("foo.ts");
    std::fs::write(&foo_path, "export class Foo {}").expect("write foo.ts");

    // index.ts — re-exports Foo from foo.ts, but nobody imports Foo from
    // index.ts anywhere. The file-wide suppress directive covers the
    // Warning.UnusedImport finding that finalize() would emit.
    let index_path = temp_dir.path().join("index.ts");
    std::fs::write(
        &index_path,
        "// cofferdam-ignore-file: Warning.UnusedImport\nexport { Foo } from './foo';\n",
    )
    .expect("write index.ts");

    let issues = engine.analyze(&[&foo_path, &index_path]).expect("analyze");

    // The directive is load-bearing — must produce neither UnusedSuppression
    // nor UnusedImport.
    let unused_suppression: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.UnusedSuppression")
        .collect();
    assert!(
        unused_suppression.is_empty(),
        "cd-wqc regression: live Warning.UnusedImport suppress directive falsely flagged as stale: {unused_suppression:#?}"
    );

    let unused_import: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.UnusedImport")
        .collect();
    assert!(
        unused_import.is_empty(),
        "suppress directive should have suppressed Warning.UnusedImport: {unused_import:#?}"
    );
}

/// Without the suppress directive the Warning.UnusedImport finding fires.
/// This guards the fixture is actually exercising the check.
#[test]
fn unused_import_fires_without_suppress_directive() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");

    let foo_path = temp_dir.path().join("foo.ts");
    std::fs::write(&foo_path, "export class Foo {}").expect("write foo.ts");

    // No suppress directive.
    let index_path = temp_dir.path().join("index.ts");
    std::fs::write(&index_path, "export { Foo } from './foo';\n").expect("write index.ts");

    let issues = engine.analyze(&[&foo_path, &index_path]).expect("analyze");

    let unused_import: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.UnusedImport")
        .collect();
    assert!(
        !unused_import.is_empty(),
        "expected Warning.UnusedImport without suppress directive"
    );
}

/// Same shape as the UnusedImport test but for `Design.OrphanExport` —
/// another finalize-only emitter. A `cofferdam-ignore-file: Design.OrphanExport`
/// directive over a file with a genuinely orphaned export must NOT produce
/// `Consistency.UnusedSuppression`.
#[test]
fn orphan_export_suppress_file_directive_not_flagged_as_stale() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");

    // bar.ts — exports Bar, but nobody imports it from anywhere.
    // The file-wide directive covers the Design.OrphanExport finding.
    let bar_path = temp_dir.path().join("bar.ts");
    std::fs::write(
        &bar_path,
        "// cofferdam-ignore-file: Design.OrphanExport\nexport class Bar {}\n",
    )
    .expect("write bar.ts");

    let issues = engine.analyze(&[&bar_path]).expect("analyze");

    let unused_suppression: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.UnusedSuppression")
        .collect();
    assert!(
        unused_suppression.is_empty(),
        "cd-wqc regression: live Design.OrphanExport suppress directive falsely flagged as stale: {unused_suppression:#?}"
    );

    let orphan_export: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Design.OrphanExport")
        .collect();
    assert!(
        orphan_export.is_empty(),
        "suppress directive should have suppressed Design.OrphanExport: {orphan_export:#?}"
    );
}

/// When there is NO actual UnusedImport finding — because the re-exported
/// symbol IS consumed — a `cofferdam-ignore-file: Warning.UnusedImport`
/// directive SHOULD be flagged as stale.  Confirms the fix doesn't break
/// the genuine stale case for finalize-only checks.
#[test]
fn suppress_directive_stale_when_finalize_only_check_has_no_finding() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");

    // foo.ts — declares Foo.
    let foo_path = temp_dir.path().join("foo.ts");
    std::fs::write(&foo_path, "export class Foo {}").expect("write foo.ts");

    // index.ts — re-exports Foo; has a suppress directive.
    let index_path = temp_dir.path().join("index.ts");
    std::fs::write(
        &index_path,
        "// cofferdam-ignore-file: Warning.UnusedImport\nexport { Foo } from './foo';\n",
    )
    .expect("write index.ts");

    // consumer.ts — imports Foo from index.ts, so the re-export IS consumed
    // and Warning.UnusedImport will not fire.
    let consumer_path = temp_dir.path().join("consumer.ts");
    std::fs::write(
        &consumer_path,
        "import { Foo } from './index';\nconst _f: Foo = new Foo();\n",
    )
    .expect("write consumer.ts");

    let issues = engine
        .analyze(&[&foo_path, &index_path, &consumer_path])
        .expect("analyze");

    let unused_suppression: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.UnusedSuppression")
        .collect();
    assert!(
        !unused_suppression.is_empty(),
        "expected Consistency.UnusedSuppression when suppress directive covers no finalize finding: {issues:#?}"
    );
}

/// Meta-test: the engine's finalize-observer dispatch (cd-9hp.5) treats
/// exactly one built-in as the observer, and it must be
/// `Consistency.UnusedSuppression`. Guards against the
/// `FINALIZE_OBSERVER_CHECK_IDS` const drifting out of sync with the
/// actual observer-style check.
#[test]
fn exactly_one_check_observes_findings() {
    let observers: Vec<&str> = all_builtins()
        .iter()
        .filter(|c| cofferdam_core::is_finalize_observer(c.meta().id))
        .map(|c| c.meta().id)
        .collect();
    assert_eq!(
        observers,
        vec!["Consistency.UnusedSuppression"],
        "expected exactly one finalize-observer check (Consistency.UnusedSuppression), got: {observers:?}"
    );
}
