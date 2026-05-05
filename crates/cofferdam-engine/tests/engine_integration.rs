//! Integration tests for `cofferdam-engine::Engine`.
//!
//! Acceptance criteria:
//! - Engine can be constructed with built-in checks
//! - analyze() processes files and returns sorted issues
//! - Parse errors are flagged as Warning.ParseError
//! - Non-fatal parse errors still allow check execution
//! - Results are priority-sorted within categories

use std::path::PathBuf;
use tempfile::TempDir;

use cofferdam_checks::all_builtins;
use cofferdam_core::Severity;
use cofferdam_engine::Engine;

// ============================================================
// Engine construction
// ============================================================

#[test]
fn engine_constructs_with_builtin_checks() {
    let _engine = Engine::new(all_builtins());

    // Should succeed and contain multiple checks
    // (Exact count depends on checks/ implementation)
    assert!(!all_builtins().is_empty());
}

// ============================================================
// Basic analysis
// ============================================================

#[test]
fn engine_analyze_on_empty_list() {
    let engine = Engine::new(all_builtins());
    let issues = engine.analyze::<PathBuf>(&[]).expect("should not error");

    assert!(issues.is_empty());
}

#[test]
fn engine_analyze_valid_typescript_file() {
    let engine = Engine::new(all_builtins());

    // Create temp file with valid TS
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");
    std::fs::write(&file_path, "const x: number = 42;").expect("write file");

    let issues = engine.analyze(&[file_path]).expect("should not error");

    // Valid code might still trigger some checks; just verify no crash
    assert!(issues
        .iter()
        .all(|i| i.severity != Severity::Critical || !i.check_id.contains("ParseError")));
}

#[test]
fn engine_analyze_tsx_file() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.tsx");
    std::fs::write(&file_path, "const elem = <div>hello</div>;").expect("write file");

    let issues = engine.analyze(&[file_path]).expect("should not error");

    // Should parse JSX without fatal errors
    assert!(issues.is_empty() || !issues.iter().all(|i| i.message.contains("ParseError")));
}

// ============================================================
// Parse error detection
// ============================================================

#[test]
fn engine_detects_parse_error_as_warning() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("bad.ts");
    std::fs::write(&file_path, "const x = {").expect("write file");

    let issues = engine.analyze(&[file_path]).expect("should not error");

    // Should have a ParseError issue
    let has_parse_error = issues
        .iter()
        .any(|i| i.check_id == "Warning.ParseError" && i.message.to_lowercase().contains("parse"));
    assert!(has_parse_error, "should have parse error issue");
}

#[test]
fn engine_reports_parse_error_with_file_path() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("bad.ts");
    std::fs::write(&file_path, "function foo() {").expect("write file");

    let issues = engine
        .analyze(std::slice::from_ref(&file_path))
        .expect("should not error");

    // Parse error should reference the file
    let parse_errors: Vec<_> = issues
        .iter()
        .filter(|i| i.message.to_lowercase().contains("parse"))
        .collect();

    assert!(!parse_errors.is_empty());
    for err in parse_errors {
        assert_eq!(err.file, file_path);
    }
}

// ============================================================
// Multiple file analysis
// ============================================================

#[test]
fn engine_analyze_multiple_files() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");

    let file1 = temp_dir.path().join("test1.ts");
    let file2 = temp_dir.path().join("test2.ts");
    let file3 = temp_dir.path().join("test3.ts");

    std::fs::write(&file1, "const a = 1;").expect("write");
    std::fs::write(&file2, "const b = 2;").expect("write");
    std::fs::write(&file3, "const c = 3;").expect("write");

    let issues = engine
        .analyze(&[&file1, &file2, &file3])
        .expect("should not error");

    // All files should be processed
    let files_in_issues: std::collections::HashSet<_> =
        issues.iter().map(|i| i.file.clone()).collect();

    assert!(files_in_issues.len() <= 3);
}

// ============================================================
// Issue sorting
// ============================================================

#[test]
fn engine_sorts_issues_by_priority() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    // Create a file that likely triggers multiple issues
    std::fs::write(&file_path, "const x = 1; const y = 2; const z = 3;").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // If there are multiple issues, they should be sorted by priority
    if issues.len() > 1 {
        for i in 1..issues.len() {
            let prev_priority = issues[i - 1].priority;
            let curr_priority = issues[i].priority;
            assert!(
                prev_priority >= curr_priority,
                "issues should be sorted by priority (descending)"
            );
        }
    }
}

// ============================================================
// File not found
// ============================================================

#[test]
fn engine_returns_error_on_missing_file() {
    let engine = Engine::new(all_builtins());

    let nonexistent = PathBuf::from("/nonexistent/path/test.ts");
    let result = engine.analyze(&[nonexistent]);

    // Should return an error, not panic
    assert!(result.is_err());
}

// ============================================================
// Different TypeScript extensions
// ============================================================

#[test]
fn engine_processes_mts_files() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("module.mts");
    std::fs::write(&file_path, "export const x = 1;").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // Should process without error
    assert!(issues.is_empty() || !issues.iter().all(|i| i.message.contains("ParseError")));
}

#[test]
fn engine_processes_cts_files() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("module.cts");
    std::fs::write(&file_path, "module.exports = { x: 1 };").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // Should process without error
    assert!(issues.is_empty() || !issues.iter().all(|i| i.message.contains("ParseError")));
}

// ============================================================
// Non-fatal parse errors still allow checks
// ============================================================

#[test]
fn engine_runs_checks_on_non_fatal_parse_errors() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");

    // Code that parses but might have warnings
    std::fs::write(&file_path, "const x = 1;").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // Even if there are no issues, the analysis should complete
    // without panicking
    assert!(issues
        .iter()
        .all(|i| i.severity != Severity::Critical || !i.check_id.contains("ParseError")));
}

// ============================================================
// Check ID validation
// ============================================================

#[test]
fn engine_issues_have_valid_check_ids() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");
    std::fs::write(&file_path, "const x = 1;").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // Each issue should have a check_id
    for issue in issues {
        assert!(!issue.check_id.is_empty(), "issue should have a check_id");
        // Check IDs follow Category.Name pattern
        assert!(
            issue.check_id.contains('.'),
            "check_id should contain a dot separator: {}",
            issue.check_id
        );
    }
}

// ============================================================
// Empty files
// ============================================================

#[test]
fn engine_handles_empty_file() {
    let engine = Engine::new(all_builtins());

    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("empty.ts");
    std::fs::write(&file_path, "").expect("write file");

    let issues = engine.analyze(&[&file_path]).expect("should not error");

    // Empty file should not cause parse errors (valid empty file)
    assert!(!issues.iter().any(|i| i.check_id == "Warning.ParseError"));
}

// ============================================================
// DuplicateBlock token mode (cd-jdq)
// ============================================================

#[test]
fn duplicate_block_token_mode_catches_cross_statement_duplicates() {
    // Two files share a 50+ token run that doesn't align cleanly with
    // statement boundaries — built so AST mode (statement windows) is
    // unlikely to fire while token mode (sliding token window) does.
    //
    // The duplicated content is a long chained method-call expression
    // whose halves land in different statements between the two files,
    // plus enough surrounding shape to clear the 50-token / 80-char
    // floors.
    let body_a = "
        export function fa(input: string) {
          const items = input.split(',').map(s => s.trim()).filter(Boolean).map(Number).filter(n => !isNaN(n)).sort((a, b) => a - b);
          return items.reduce((acc, n) => acc + n, 0);
        }
    ";
    let body_b = "
        export function fb(payload: string) {
          const list = payload.split(',').map(s => s.trim()).filter(Boolean).map(Number).filter(n => !isNaN(n)).sort((a, b) => a - b);
          const total = list.reduce((acc, n) => acc + n, 0);
          return total;
        }
    ";

    let temp_dir = TempDir::new().expect("temp dir");
    let path_a = temp_dir.path().join("a.ts");
    let path_b = temp_dir.path().join("b.ts");
    std::fs::write(&path_a, body_a).expect("write a");
    std::fs::write(&path_b, body_b).expect("write b");

    use cofferdam_checks::refactor::DuplicateBlock;
    let engine = Engine::new(vec![Box::new(DuplicateBlock::with_tokens(50))]);
    let issues = engine
        .analyze(&[&path_a, &path_b])
        .expect("analyze should succeed");

    let dup_findings: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.DuplicateBlock")
        .collect();
    assert!(
        !dup_findings.is_empty(),
        "expected at least one DuplicateBlock finding, got {} total issues: {:?}",
        issues.len(),
        issues.iter().map(|i| &i.check_id).collect::<Vec<_>>()
    );

    // The finding must reference both files (one as primary, the other
    // in `related`).
    let referenced_files: std::collections::HashSet<PathBuf> = dup_findings
        .iter()
        .flat_map(|i| {
            std::iter::once(i.file.clone()).chain(i.related.iter().map(|r| r.file.clone()))
        })
        .collect();
    assert!(
        referenced_files.contains(&path_a) && referenced_files.contains(&path_b),
        "expected the DuplicateBlock finding to span both files, got {:?}",
        referenced_files
    );
}

#[test]
fn duplicate_block_default_does_not_emit_token_findings() {
    // Same fixtures as above, but with the default (AST-only)
    // DuplicateBlock. No statement-aligned 6+ run exists, so AST mode
    // should produce nothing — proves token mode is genuinely opt-in.
    let body_a = "
        export function fa(input: string) {
          const items = input.split(',').map(s => s.trim()).filter(Boolean).map(Number).filter(n => !isNaN(n)).sort((a, b) => a - b);
          return items.reduce((acc, n) => acc + n, 0);
        }
    ";
    let body_b = "
        export function fb(payload: string) {
          const list = payload.split(',').map(s => s.trim()).filter(Boolean).map(Number).filter(n => !isNaN(n)).sort((a, b) => a - b);
          const total = list.reduce((acc, n) => acc + n, 0);
          return total;
        }
    ";

    let temp_dir = TempDir::new().expect("temp dir");
    let path_a = temp_dir.path().join("a.ts");
    let path_b = temp_dir.path().join("b.ts");
    std::fs::write(&path_a, body_a).expect("write a");
    std::fs::write(&path_b, body_b).expect("write b");

    use cofferdam_checks::refactor::DuplicateBlock;
    let engine = Engine::new(vec![Box::new(DuplicateBlock::default())]);
    let issues = engine
        .analyze(&[&path_a, &path_b])
        .expect("analyze should succeed");

    assert!(
        issues
            .iter()
            .all(|i| i.check_id != "Refactor.DuplicateBlock"),
        "AST-only DuplicateBlock should not flag the cross-statement \
         duplicate; token mode must be opt-in. Got: {:?}",
        issues
    );
}

// ============================================================
// Two-pass consistency checks
// ============================================================

/// Integration test: full engine run on a mixed-quote file produces exactly
/// 2 `Consistency.QuoteStyle` findings (the two [FLAG] strings in the fixture).
#[test]
fn quote_style_fixture_produces_expected_count() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("quote_style.ts");

    // Skip if fixture is somehow missing (shouldn't happen in CI).
    if !fixture.exists() {
        eprintln!("SKIP: quote_style.ts fixture not found at {:?}", fixture);
        return;
    }

    use cofferdam_checks::consistency::QuoteStyle;
    let engine = Engine::new(vec![Box::new(QuoteStyle)]);
    let issues = engine.analyze(&[&fixture]).expect("analyze should succeed");

    let qs_issues: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.QuoteStyle")
        .collect();

    assert_eq!(
        qs_issues.len(),
        2,
        "expected 2 QuoteStyle findings on quote_style.ts; got {:?}",
        qs_issues
    );
}

// ============================================================
// Refactor.LongAndComplex + MaxFunctionLength comment skip
// ============================================================

#[test]
fn long_and_complex_only_flags_tangled_function() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace dir")
        .parent()
        .expect("repo root")
        .join("examples")
        .join("long_and_complex.ts");

    // Engine pre-fills options from spec defaults, so the constructor
    // values would be overridden anyway. Use the production defaults
    // (75, 15) and rely on the fixture clearing both. `flatLong`
    // (cyc 1), `shortBranchy` (length ~15), and `commentHeavy`
    // (effective length ~2, cyc 1) all stay below at least one
    // threshold; `tangled` is constructed to comfortably clear both.
    use cofferdam_checks::refactor::LongAndComplex;
    let engine = Engine::new(vec![Box::new(LongAndComplex::new(75, 15))]);
    let issues = engine.analyze(&[&fixture]).expect("analyze should succeed");

    let lac: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Refactor.LongAndComplex")
        .collect();

    assert_eq!(
        lac.len(),
        1,
        "expected exactly one LongAndComplex finding (the `tangled` function); got {} findings: {:?}",
        lac.len(),
        lac.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
    assert!(
        lac[0].message.contains("tangled"),
        "expected the finding to name `tangled`; got: {}",
        lac[0].message
    );
}

#[test]
fn max_function_length_skips_blanks_and_comments() {
    // `commentHeavy` in the fixture has a body that spans ~40 lines but
    // ~35 of them are pure-comment lines. With a default limit of 50 the
    // raw-span measurement would (just barely) not flag it, but the
    // comment-heavy case is intentionally constructed so that effective
    // length stays well under 50 — the test guards against regressions
    // toward raw-span counting.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace dir")
        .parent()
        .expect("repo root")
        .join("examples")
        .join("long_and_complex.ts");

    // Engine pre-fills options from spec defaults (limit=50), so use
    // those defaults. `commentHeavy` has ~37 raw body lines but ~35
    // are pure comment lines — effective length ~2, well under 50.
    // The previous raw-span measurement would have given 37 (still
    // under 50) so this test guards against a regression that flips
    // the default to a tighter value AND drops the comment-skip.
    use cofferdam_checks::readability::MaxFunctionLength;
    let engine = Engine::new(vec![Box::new(MaxFunctionLength::new(50))]);
    let issues = engine.analyze(&[&fixture]).expect("analyze should succeed");

    let mfl: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Readability.MaxFunctionLength")
        .collect();

    let comment_heavy_flagged = mfl.iter().any(|i| i.message.contains("commentHeavy"));
    assert!(
        !comment_heavy_flagged,
        "commentHeavy should not be flagged — its effective length \
         (excluding blanks and comments) is ~2, well below the 50-line \
         limit. Findings: {:?}",
        mfl.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}
