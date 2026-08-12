//! Integration tests for suppression directives (cd-5t7).

use tempfile::TempDir;

use cofferdam_checks::all_builtins;
use cofferdam_engine::Engine;

#[test]
fn suppress_next_line_all_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line\nlet a = 1;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have no PreferConstOverLet issue on line 2 due to suppression
    let prefer_const_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 2)
        .collect();
    assert!(
        prefer_const_on_line_2.is_empty(),
        "expected no PreferConstOverLet on line 2 (should be suppressed)"
    );
}

#[test]
fn suppress_next_line_specific_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line Refactor.PreferConstOverLet\nlet a = 1;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have no PreferConstOverLet on line 2
    let prefer_const_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 2)
        .collect();
    assert!(prefer_const_on_line_2.is_empty());
}

#[test]
fn suppress_next_line_skips_blanks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-disable-next-line\n\nlet a = 1;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Blank line 2 should not suppress anything
    // Line 3 (first non-blank) should be suppressed
    let prefer_const_on_line_3: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 3)
        .collect();
    assert!(
        prefer_const_on_line_3.is_empty(),
        "expected no PreferConstOverLet on line 3 (suppressed by line 1 directive)"
    );
}

#[test]
fn suppress_block_all_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "/* cofferdam-disable */\nlet a = 1;\n/* cofferdam-enable */\nlet x = 2;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Line 2 should be suppressed
    let prefer_const_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 2)
        .collect();
    assert!(
        prefer_const_on_line_2.is_empty(),
        "expected no PreferConstOverLet on line 2 (inside disable block)"
    );

    // Line 4 should NOT be suppressed
    let prefer_const_on_line_4: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 4)
        .collect();
    assert!(
        !prefer_const_on_line_4.is_empty(),
        "expected PreferConstOverLet on line 4 (outside disable block)"
    );
}

#[test]
fn suppress_block_specific_checks() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code =
        "/* cofferdam-disable Refactor.PreferConstOverLet */\nlet a = 1;\n/* cofferdam-enable */";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Line 2 should suppress PreferConstOverLet but not other checks
    let prefer_const_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 2)
        .collect();
    assert!(
        prefer_const_on_line_2.is_empty(),
        "expected no PreferConstOverLet on line 2"
    );
}

#[test]
fn suppress_block_no_matching_enable() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "/* cofferdam-disable Refactor.PreferConstOverLet */\nlet a = 1;\nlet x = 2;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Both lines 2 and 3 should be suppressed (extends to EOF)
    let prefer_const_on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 2)
        .collect();
    let prefer_const_on_line_3: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet" && i.location.line() == 3)
        .collect();
    assert!(
        prefer_const_on_line_2.is_empty(),
        "line 2 should be suppressed"
    );
    assert!(
        prefer_const_on_line_3.is_empty(),
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
fn without_suppression_prefer_const_fires() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "let a = 1;";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    // Should have a PreferConstOverLet issue on line 1
    let prefer_const: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.PreferConstOverLet")
        .collect();
    assert!(
        !prefer_const.is_empty(),
        "expected at least one PreferConstOverLet issue without suppression"
    );
}

// ============================================================
// cd-wqc regression: finalize-only emitters + UnusedSuppression
// ============================================================

/// `Design.OrphanExport` is a finalize-only emitter. A
/// `cofferdam-ignore-file: Design.OrphanExport` directive over a file with
/// a genuinely orphaned export must NOT produce `Consistency.UnusedSuppression`.
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
/// export IS consumed — a `cofferdam-ignore-file: Design.OrphanExport`
/// directive SHOULD be flagged as stale.  Confirms the fix doesn't break
/// the genuine stale case for finalize-only checks.
#[test]
fn suppress_directive_stale_when_finalize_only_check_has_no_finding() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");

    // foo.ts — declares Foo, with a directive claiming to suppress
    // Design.OrphanExport.
    let foo_path = temp_dir.path().join("foo.ts");
    std::fs::write(
        &foo_path,
        "// cofferdam-ignore-file: Design.OrphanExport\nexport class Foo {}\n",
    )
    .expect("write foo.ts");

    // consumer.ts — imports Foo from foo.ts, so the export IS consumed
    // and Design.OrphanExport will not fire.
    let consumer_path = temp_dir.path().join("consumer.ts");
    std::fs::write(
        &consumer_path,
        "import { Foo } from './foo';\nconst _f: Foo = new Foo();\n",
    )
    .expect("write consumer.ts");

    let issues = engine
        .analyze(&[&foo_path, &consumer_path])
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
