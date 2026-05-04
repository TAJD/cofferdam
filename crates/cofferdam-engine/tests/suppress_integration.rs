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
// Biome-style suppressions on real built-in checks (cd-81a.4)
// ============================================================

#[test]
fn biome_ignore_with_id_silences_engine_finding() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    // Without the directive this would emit Warning.TripleEquals on line 2.
    let code = "// cofferdam-ignore: Warning.TripleEquals\nif (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    let on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(on_line_2.is_empty(), "Biome-style ignore should suppress");
}

#[test]
fn biome_ignore_file_silences_whole_file() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code = "// cofferdam-ignore-file: Warning.TripleEquals\nif (a == b) { }\nif (x == y) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    let triple_eq: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals")
        .collect();
    assert!(
        triple_eq.is_empty(),
        "file-wide ignore should silence every TripleEquals match"
    );
}

#[test]
fn biome_ignore_with_reason_strips_reason_and_still_works() {
    let engine = Engine::new(all_builtins());
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    let code =
        "// cofferdam-ignore: Warning.TripleEquals: legacy code, see cd-XYZ\nif (a == b) { }";
    std::fs::write(&file_path, code).expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("analyze");

    let on_line_2: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TripleEquals" && i.span.line == 2)
        .collect();
    assert!(
        on_line_2.is_empty(),
        "ignore with reason should still match — reason text is parsed but discarded"
    );
}
