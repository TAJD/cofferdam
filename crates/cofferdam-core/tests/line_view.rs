//! Integration tests for the LineView API (cd-81a.1).
//!
//! Acceptance:
//! - `lines()` returns iterable with all flags populated
//! - block comments correctly classify all spanned lines
//! - template-literal lines are flagged
//!
//! Plus the implicit cases (line comments, JSDoc, single-line strings,
//! pragmas, no-classification lines, CRLF stripping).

use std::path::PathBuf;

use cofferdam_core::parser::{parse_into, ParsedView};
use cofferdam_core::{Allocator, AstView, CheckContext, LineView, SourceFile};

fn view_for(text: &'static str) -> AstView<'static> {
    let path = PathBuf::from("test.ts");
    let file = Box::leak(Box::new(SourceFile::new(path, text.to_string())));
    let allocator: &'static Allocator = Box::leak(Box::new(Allocator::default()));
    let parsed_return = parse_into(allocator, file);
    let parsed: &'static ParsedView<'static> = Box::leak(Box::new(ParsedView {
        program: Box::leak(Box::new(parsed_return.program)),
        diagnostics: Box::leak(parsed_return.errors.into_boxed_slice()),
    }));
    CheckContext::new(file)
        .with_parsed(parsed)
        .ast()
        .expect("parsed view should produce AstView")
}

fn collect(view: &AstView<'static>) -> Vec<LineView<'static>> {
    view.lines().collect()
}

// ============================================================
// Plain code — every flag false on every line
// ============================================================

#[test]
fn plain_code_has_no_classification() {
    let view = view_for(
        r#"const a = 1;
const b = 2;
const c = a + b;"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 3);
    for (i, l) in lines.iter().enumerate() {
        assert_eq!(l.line_no, i as u32 + 1);
        assert!(
            !l.is_comment && !l.is_doc_comment && !l.is_string_literal && !l.is_pragma,
            "line {} should be unclassified, got {:?}",
            l.line_no,
            l
        );
    }
    assert_eq!(lines[0].text, "const a = 1;");
    assert_eq!(lines[2].text, "const c = a + b;");
}

// ============================================================
// Single-line `//` comment
// ============================================================

#[test]
fn line_comment_marks_only_its_line() {
    let view = view_for(
        r#"// just a comment
const a = 1;
const b = 2; // trailing
const c = 3;"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 4);

    assert!(lines[0].is_comment, "line 1 is //");
    assert!(!lines[0].is_doc_comment);
    assert!(!lines[0].is_pragma);

    assert!(!lines[1].is_comment, "plain const line");

    assert!(lines[2].is_comment, "trailing comment touches its line");
    assert!(!lines[3].is_comment);
}

// ============================================================
// JSDoc — is_doc_comment AND is_comment
// ============================================================

#[test]
fn jsdoc_is_doc_comment() {
    let view = view_for(
        r#"/** docs for foo */
function foo() {}"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].is_comment);
    assert!(lines[0].is_doc_comment, "JSDoc should set is_doc_comment");
    assert!(!lines[1].is_doc_comment);
}

#[test]
fn multiline_jsdoc_marks_every_line() {
    let view = view_for(
        r#"/**
 * Multi-line JSDoc.
 * Second line of docs.
 */
function foo() {}"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 5);
    for l in lines.iter().take(4) {
        assert!(l.is_comment, "line {} should be comment", l.line_no);
        assert!(l.is_doc_comment, "line {} should be doc", l.line_no);
    }
    assert!(!lines[4].is_comment);
}

// ============================================================
// Multi-line block comment — every spanned line marked is_comment
// (acceptance criterion: block comments classify all spanned lines)
// ============================================================

#[test]
fn block_comment_classifies_every_spanned_line() {
    let view = view_for(
        r#"const a = 1;
/* block
   spanning
   three */
const b = 2;"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 5);

    assert!(!lines[0].is_comment, "code before");
    assert!(lines[1].is_comment, "/* opens here");
    assert!(lines[2].is_comment, "block continues");
    assert!(lines[3].is_comment, "*/ closes here");
    assert!(!lines[4].is_comment, "code after");

    // None of these are JSDoc.
    for l in &lines[1..=3] {
        assert!(!l.is_doc_comment);
    }
}

// ============================================================
// Pragma — `is_annotation` style comments
// ============================================================

#[test]
fn pragma_comments_set_is_pragma() {
    let view = view_for(
        r#"/* @vite-ignore */
import("./module.ts");
const x = /* #__PURE__ */ Object.freeze({});"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].is_comment);
    assert!(lines[0].is_pragma, "@vite-ignore is a pragma");

    assert!(lines[2].is_comment);
    assert!(lines[2].is_pragma, "#__PURE__ is a pragma");
}

#[test]
fn jsdoc_is_not_pragma() {
    let view = view_for("/** docs */\nfunction foo() {}");
    let lines = collect(&view);
    assert!(lines[0].is_doc_comment);
    assert!(
        !lines[0].is_pragma,
        "JSDoc is doc, not pragma — they're different axes"
    );
}

// ============================================================
// Single-line string literal
// ============================================================

#[test]
fn single_line_string_marks_its_line() {
    let view = view_for(
        r#"const a = "hello";
const b = 2;
const c = 'world';"#,
    );
    let lines = collect(&view);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].is_string_literal);
    assert!(!lines[1].is_string_literal);
    assert!(lines[2].is_string_literal);
}

// ============================================================
// Multi-line template literal (acceptance criterion: template lines
// flagged so user-facing strings inside backticks don't false-positive
// brand checks).
// ============================================================

#[test]
fn template_literal_marks_every_line() {
    let view = view_for("const t = `line one\nline two\nline three`;\nconst end = 0;");
    let lines = collect(&view);
    assert_eq!(lines.len(), 4);

    assert!(lines[0].is_string_literal, "backtick opens here");
    assert!(lines[1].is_string_literal, "interior of template");
    assert!(lines[2].is_string_literal, "backtick closes here");
    assert!(!lines[3].is_string_literal, "code after");
}

#[test]
fn template_with_interpolation_still_marks_every_line() {
    let view = view_for("const t = `foo\n${bar}\nbaz`;");
    let lines = collect(&view);
    assert_eq!(lines.len(), 3);
    for l in &lines {
        assert!(
            l.is_string_literal,
            "template line {} should be flagged",
            l.line_no
        );
    }
}

// ============================================================
// CRLF + edge cases
// ============================================================

#[test]
fn crlf_is_stripped_from_text() {
    // Hand-crafted CRLF source (raw strings can't easily include CRLF
    // on every editor — build it explicitly here).
    let path = PathBuf::from("test.ts");
    let text = "const a = 1;\r\nconst b = 2;\r\n";
    let file = Box::leak(Box::new(SourceFile::new(path, text.to_string())));
    let allocator: &'static Allocator = Box::leak(Box::new(Allocator::default()));
    let parsed_return = parse_into(allocator, file);
    let parsed: &'static ParsedView<'static> = Box::leak(Box::new(ParsedView {
        program: Box::leak(Box::new(parsed_return.program)),
        diagnostics: Box::leak(parsed_return.errors.into_boxed_slice()),
    }));
    let view = CheckContext::new(file).with_parsed(parsed).ast().unwrap();

    let lines: Vec<_> = view.lines().collect();
    // Two content lines + one trailing empty (matches `split('\n')`).
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].text, "const a = 1;");
    assert_eq!(lines[1].text, "const b = 2;");
    assert_eq!(lines[2].text, "");
}

#[test]
fn empty_file_yields_one_empty_line() {
    let view = view_for("");
    let lines = collect(&view);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].line_no, 1);
    assert_eq!(lines[0].text, "");
    assert!(!lines[0].is_comment);
    assert!(!lines[0].is_string_literal);
}

#[test]
fn file_without_trailing_newline_has_no_phantom_line() {
    let view = view_for("const a = 1;");
    let lines = collect(&view);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "const a = 1;");
}

// ============================================================
// Line numbers are 1-based and contiguous
// ============================================================

#[test]
fn line_numbers_are_one_based_contiguous() {
    let view = view_for("a;\nb;\nc;\nd;");
    let nos: Vec<u32> = view.lines().map(|l| l.line_no).collect();
    assert_eq!(nos, vec![1, 2, 3, 4]);
}

// ============================================================
// Multiple flags can co-occur — string + comment + pragma on one line.
// JSDoc is dropped because oxc only tags `/** */` blocks as JSDoc
// when they're in a leading-token position, which an inline comment
// isn't — that's an oxc-side detail, not a flag-orthogonality issue.
// ============================================================

#[test]
fn flags_compose_on_a_single_line() {
    let view = view_for(r#"const s = "x"; /* @vite-ignore */ // trailing"#);
    let lines = collect(&view);
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert!(l.is_comment, "line has comments");
    assert!(l.is_pragma, "vite-ignore on line");
    assert!(l.is_string_literal, "string on line");
}

// ============================================================
// CD-100: string_literal_ranges is span-aware, unlike the whole-line
// is_string_literal flag.
// ============================================================

#[test]
fn string_literal_ranges_covers_only_the_literal_span() {
    let view = view_for(r#"const src = "/x.png"; const tag = "<img>";"#);
    let lines = collect(&view);
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert!(l.is_string_literal, "whole-line flag still set");
    assert_eq!(l.string_literal_ranges.len(), 2, "two literals on the line");
    let (s0, e0) = l.string_literal_ranges[0];
    assert_eq!(&l.text[s0 as usize..e0 as usize], r#""/x.png""#);
    let (s1, e1) = l.string_literal_ranges[1];
    assert_eq!(&l.text[s1 as usize..e1 as usize], r#""<img>""#);
}

#[test]
fn string_literal_ranges_excludes_non_literal_text_sharing_the_line() {
    // The ticket's repro: a tag sharing a line with a string-valued
    // attribute must not have the tag portion counted as a literal
    // range, even though is_string_literal is true for the whole line.
    let view = view_for(r#"const html = tag + "/x.png";"#);
    let lines = collect(&view);
    let l = &lines[0];
    let (s, e) = l.string_literal_ranges[0];
    let literal_text = &l.text[s as usize..e as usize];
    assert_eq!(literal_text, r#""/x.png""#);
    assert!(
        !l.text[..s as usize].is_empty(),
        "non-literal prefix exists and is outside the range"
    );
}

#[test]
fn string_literal_ranges_empty_when_no_literal() {
    let view = view_for("const a = 1;");
    let lines = collect(&view);
    assert!(lines[0].string_literal_ranges.is_empty());
}

#[test]
fn plain_lines_have_no_string_literal_ranges() {
    let lines: Vec<_> = cofferdam_core::Lines::plain("some\ntext").collect();
    for l in &lines {
        assert!(l.string_literal_ranges.is_empty());
        assert!(!l.is_string_literal);
    }
}
