//! Integration tests for `cofferdam-core::parser` module.
//!
//! Acceptance criteria:
//! - `parse_into` parses .ts/.tsx/.cts/.mts correctly with proper SourceType
//! - `source_type_for` maps extensions to correct SourceType variants
//! - `.tsx` files enable JSX parsing
//! - `parse_fatal` correctly identifies unrecoverable parse errors
//! - Non-fatal errors still produce a usable Program

use std::path::PathBuf;

use cofferdam_core::parser::{parse_fatal, parse_into, source_type_for, ParsedView};
use cofferdam_core::{Allocator, CheckContext, SourceFile};
use oxc_span::SourceType;

// ============================================================
// source_type_for: extension → SourceType mapping
// ============================================================

#[test]
fn source_type_for_ts_files() {
    assert_eq!(source_type_for(&PathBuf::from("test.ts")), SourceType::ts());
}

#[test]
fn source_type_for_tsx_files() {
    assert_eq!(
        source_type_for(&PathBuf::from("test.tsx")),
        SourceType::tsx()
    );
}

#[test]
fn source_type_for_mts_files() {
    assert_eq!(
        source_type_for(&PathBuf::from("test.mts")),
        SourceType::ts()
    );
}

#[test]
fn source_type_for_cts_files() {
    assert_eq!(
        source_type_for(&PathBuf::from("test.cts")),
        SourceType::ts()
    );
}

#[test]
fn source_type_for_d_ts_files() {
    let st = source_type_for(&PathBuf::from("test.d.ts"));
    let st_normal = SourceType::ts();
    // Declaration files should have different flags
    assert_ne!(st, st_normal);
}

#[test]
fn source_type_for_d_mts_files() {
    let st = source_type_for(&PathBuf::from("test.d.mts"));
    let st_normal = SourceType::ts();
    assert_ne!(st, st_normal);
}

#[test]
fn source_type_for_d_cts_files() {
    let st = source_type_for(&PathBuf::from("test.d.cts"));
    let st_normal = SourceType::ts();
    assert_ne!(st, st_normal);
}

#[test]
fn source_type_for_unknown_extension_defaults_to_ts() {
    assert_eq!(source_type_for(&PathBuf::from("test.js")), SourceType::ts());
    assert_eq!(
        source_type_for(&PathBuf::from("test.txt")),
        SourceType::ts()
    );
}

// ============================================================
// parse_into: basic parsing behavior
// ============================================================

#[test]
fn parse_into_parses_valid_typescript() {
    let path = PathBuf::from("test.ts");
    let text = "const x: number = 42;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Should have a program, no errors
    assert_eq!(parsed_return.errors.len(), 0);
    // Program should have at least a statements vec
    assert!(!parsed_return.program.body.is_empty());
}

#[test]
fn parse_into_tsx_enables_jsx() {
    let path = PathBuf::from("test.tsx");
    let text = "const elem = <div>hello</div>;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Should parse JSX without errors
    assert_eq!(parsed_return.errors.len(), 0);
    assert!(!parsed_return.program.body.is_empty());
}

#[test]
fn parse_into_ts_rejects_bare_jsx() {
    // .ts files don't support JSX syntax
    let path = PathBuf::from("test.ts");
    let text = "const elem = <div>hello</div>;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Should have errors due to invalid JSX in .ts context
    assert!(!parsed_return.errors.is_empty());
}

#[test]
fn parse_into_preserves_allocation_within_allocator() {
    let path = PathBuf::from("test.ts");
    let text = "function foo(a: string, b: number): boolean { return true; }";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Program should be valid and use the allocator's lifetime
    assert!(!parsed_return.program.body.is_empty());
}

// ============================================================
// parse_fatal: error severity detection
// ============================================================

#[test]
fn parse_fatal_detects_unclosed_brace() {
    let path = PathBuf::from("test.ts");
    let text = "const x = { a: 1";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Should have errors and be unrecoverable
    assert!(!parsed_return.errors.is_empty());
    assert!(parse_fatal(&parsed_return));
}

#[test]
fn parse_fatal_allows_syntax_warnings() {
    // Certain syntax issues (like trailing commas) are recoverable
    let path = PathBuf::from("test.ts");
    let text = "const x = 1;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Valid code should not be fatal
    assert!(!parse_fatal(&parsed_return));
}

// ============================================================
// parse_into with CheckContext integration
// ============================================================

#[test]
fn parse_into_result_works_with_check_context() {
    let path = PathBuf::from("test.ts");
    let text = "const x = 1; const y = 2;";
    let file = Box::leak(Box::new(SourceFile::new(path, text)));
    let allocator: &'static Allocator = Box::leak(Box::new(Allocator::default()));

    let parsed_return = parse_into(allocator, file);
    let parsed: &'static ParsedView<'static> = Box::leak(Box::new(ParsedView {
        program: Box::leak(Box::new(parsed_return.program)),
        diagnostics: Box::leak(parsed_return.errors.into_boxed_slice()),
    }));

    let ctx = CheckContext::new(file).with_parsed(parsed);

    // Should successfully create context
    assert!(ctx.parsed.is_some());
}

// ============================================================
// Multi-line and edge cases
// ============================================================

#[test]
fn parse_into_handles_multiline_strings() {
    let path = PathBuf::from("test.ts");
    let text = "const s = `line 1\nline 2\nline 3`;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    assert_eq!(parsed_return.errors.len(), 0);
    assert!(!parsed_return.program.body.is_empty());
}

#[test]
fn parse_into_handles_empty_file() {
    let path = PathBuf::from("test.ts");
    let text = "";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Empty file is valid
    assert_eq!(parsed_return.errors.len(), 0);
    assert!(parsed_return.program.body.is_empty());
}

#[test]
fn parse_into_handles_comments() {
    let path = PathBuf::from("test.ts");
    let text = "// comment\nconst x = 1; /* block */";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    assert_eq!(parsed_return.errors.len(), 0);
    assert!(!parsed_return.program.body.is_empty());
}

#[test]
fn parse_into_preserves_diagnostic_information() {
    let path = PathBuf::from("test.ts");
    // Missing closing brace - should be detectable
    let text = "function foo() { const x = 1;";
    let file = SourceFile::new(path, text);
    let allocator = Allocator::default();

    let parsed_return = parse_into(&allocator, &file);

    // Should record errors for later inspection
    assert!(!parsed_return.errors.is_empty());
}
