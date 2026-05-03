//! Plugin-facing line view + classification (cd-81a.1).
//!
//! `AstView::lines()` yields a [`LineView`] for every textual line in
//! the source, with classification flags populated from the parsed
//! comment list and an AST walk over string/template literals.
//!
//! Why this isn't text-only: a regex like `^\s*//` misses block
//! comments split across lines, JSDoc, and pragmas — and a regex over
//! quote characters misclassifies escape sequences and template
//! interpolations. oxc has already paid the parse cost; we lift its
//! comment table and a one-pass literal walk to get accurate flags
//! for free.
//!
//! # Flag semantics
//!
//! Each flag is true when the line is **touched** by the named
//! construct — a multi-line block comment marks every line it spans;
//! a template literal marks every line of the backtick-enclosed text
//! including embedded `${expr}` lines.
//!
//! - `is_comment` — any comment (line `//`, single-line block, or
//!   multi-line block) overlaps this line.
//! - `is_doc_comment` — a JSDoc-style block (`/** ... */`) overlaps.
//!   Implies `is_comment`.
//! - `is_string_literal` — a `StringLiteral` or `TemplateLiteral` span
//!   overlaps this line.
//! - `is_jsx_text` — a `JSXText` span overlaps this line. JSX text is
//!   the user-facing copy *between* JSX tags (`<p>Hello</p>` → "Hello"),
//!   not attribute values or expression containers. Plugin checks that
//!   target display copy (BrandCasing in cd-7e4) flag this alongside
//!   `is_string_literal`. Filed as cd-0ne.
//! - `is_pragma` — an annotation-style comment overlaps. Pragmas are
//!   compiler-hint comments like `/* #__PURE__ */`, `/* @vite-ignore */`,
//!   `/* webpackChunkName: "x" */` — *not* JSDoc and *not* legal
//!   headers. Maps to `oxc_ast::Comment::is_annotation()`.

use oxc_ast::ast::{Comment, JSXText, StringLiteral, TemplateLiteral};
use oxc_ast_visit::{walk, Visit};
use oxc_span::Span as OxcSpan;
use oxc_syntax::scope::ScopeFlags;

use crate::issue::Span;

/// One line of source plus classification flags.
#[derive(Debug, Clone, Copy)]
pub struct LineView<'a> {
    /// 1-based line number.
    pub line_no: u32,
    /// Line text with the trailing `\r` (CRLF) stripped, no `\n`.
    pub text: &'a str,
    pub is_comment: bool,
    pub is_doc_comment: bool,
    pub is_string_literal: bool,
    pub is_jsx_text: bool,
    pub is_pragma: bool,
    /// 0-based byte offset of the start of this line in the full source
    /// text. Drives [`span_for`](Self::span_for) — kept public so AST
    /// checks that have already computed offsets can build spans
    /// without re-walking the line table.
    pub line_start: u32,
}

impl LineView<'_> {
    /// Build a [`Span`] covering bytes `[char_start, char_end)` *within
    /// this line*. Both arguments are byte offsets relative to
    /// `self.text` (after CRLF stripping); the returned span carries
    /// file-absolute `start_byte`/`end_byte` and 1-based line/column,
    /// ready for `Issue.span` or plugin `ctx.report`.
    ///
    /// Filed as cd-cgd — keeps line-walk plugin authoring concise.
    pub fn span_for(&self, char_start: u32, char_end: u32) -> Span {
        Span {
            line: self.line_no,
            column: char_start + 1,
            start_byte: self.line_start + char_start,
            end_byte: self.line_start + char_end,
        }
    }
}

/// Iterator over [`LineView`]s for an `AstView`. Returned by
/// [`crate::AstView::lines`].
pub struct Lines<'a> {
    text: &'a str,
    /// Byte offset of the start of each line. `len() == number of lines`.
    line_starts: Vec<u32>,
    flags: Vec<LineFlags>,
    idx: usize,
}

impl<'a> Lines<'a> {
    pub(crate) fn build(text: &'a str, program: &'a oxc_ast::ast::Program<'a>) -> Self {
        let line_starts = compute_line_starts(text);
        let mut flags = vec![LineFlags::default(); line_starts.len().max(1)];

        for c in &program.comments {
            apply_comment(&line_starts, &mut flags, c);
        }

        let mut literal = LiteralCollector {
            string_spans: Vec::new(),
            jsx_text_spans: Vec::new(),
        };
        literal.visit_program(program);
        for sp in literal.string_spans {
            apply_span(&line_starts, &mut flags, sp, |f| f.is_string_literal = true);
        }
        for sp in literal.jsx_text_spans {
            apply_span(&line_starts, &mut flags, sp, |f| f.is_jsx_text = true);
        }

        Self {
            text,
            line_starts,
            flags,
            idx: 0,
        }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = LineView<'a>;

    fn next(&mut self) -> Option<LineView<'a>> {
        if self.idx >= self.line_starts.len() {
            return None;
        }
        let i = self.idx;
        self.idx += 1;

        let start = self.line_starts[i] as usize;
        // End of line content = byte before next line's '\n', or end of text.
        let raw_end = if i + 1 < self.line_starts.len() {
            // line_starts[i + 1] points just AFTER the '\n', so the
            // newline byte is at line_starts[i + 1] - 1. Trim it.
            (self.line_starts[i + 1] as usize).saturating_sub(1)
        } else {
            self.text.len()
        };
        let raw = &self.text[start..raw_end.min(self.text.len())];
        let text = raw.strip_suffix('\r').unwrap_or(raw);

        let f = self.flags[i];
        Some(LineView {
            line_no: i as u32 + 1,
            text,
            is_comment: f.is_comment,
            is_doc_comment: f.is_doc_comment,
            is_string_literal: f.is_string_literal,
            is_jsx_text: f.is_jsx_text,
            is_pragma: f.is_pragma,
            line_start: self.line_starts[i],
        })
    }
}

#[derive(Default, Clone, Copy)]
struct LineFlags {
    is_comment: bool,
    is_doc_comment: bool,
    is_string_literal: bool,
    is_jsx_text: bool,
    is_pragma: bool,
}

/// Indices into the source where each line starts. Always begins with 0;
/// subsequent entries point one byte past every `\n`.
fn compute_line_starts(text: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(text.bytes().filter(|&b| b == b'\n').count() + 1);
    starts.push(0);
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            // Line after the '\n' starts at i+1. May equal text.len()
            // if the file ends with a newline; that yields an empty
            // trailing line, matching `text.split('\n')` semantics.
            starts.push(i as u32 + 1);
        }
    }
    starts
}

/// Largest line index `i` such that `line_starts[i] <= byte`.
fn byte_to_line(line_starts: &[u32], byte: u32) -> u32 {
    match line_starts.binary_search(&byte) {
        Ok(i) => i as u32,
        // Err(i) means byte fits between starts[i-1] and starts[i].
        // Saturate so a byte before line 1 (impossible in practice)
        // still maps to line 0.
        Err(i) => (i.saturating_sub(1)) as u32,
    }
}

fn apply_span(
    line_starts: &[u32],
    flags: &mut [LineFlags],
    span: OxcSpan,
    mut set: impl FnMut(&mut LineFlags),
) {
    let start_line = byte_to_line(line_starts, span.start);
    // Span end is exclusive — the last byte covered is end-1. For an
    // empty span (start == end) treat it as touching `start`.
    let last_byte = span.end.saturating_sub(1).max(span.start);
    let end_line = byte_to_line(line_starts, last_byte);
    for li in start_line..=end_line {
        if let Some(f) = flags.get_mut(li as usize) {
            set(f);
        }
    }
}

fn apply_comment(line_starts: &[u32], flags: &mut [LineFlags], c: &Comment) {
    let is_jsdoc = c.is_jsdoc();
    let is_annotation = c.is_annotation();
    apply_span(line_starts, flags, c.span, |f| {
        f.is_comment = true;
        if is_jsdoc {
            f.is_doc_comment = true;
        }
        if is_annotation {
            f.is_pragma = true;
        }
    });
}

// ---- internal: AST walk for string + template + JSX text spans -----

struct LiteralCollector {
    string_spans: Vec<OxcSpan>,
    jsx_text_spans: Vec<OxcSpan>,
}

impl<'a> Visit<'a> for LiteralCollector {
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        self.string_spans.push(it.span);
    }
    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        self.string_spans.push(it.span);
        // Descend so nested templates inside `${...}` interpolations
        // are still recorded.
        walk::walk_template_literal(self, it);
    }
    fn visit_jsx_text(&mut self, it: &JSXText<'a>) {
        // JSXText spans are the user-facing copy *between* JSX tags.
        // Attribute values (`title="..."`) are StringLiterals inside
        // the attribute, picked up by visit_string_literal — not
        // double-counted here.
        self.jsx_text_spans.push(it.span);
    }
    // Keep descending the rest of the tree — the auto-walking impls
    // are fine, they'd only be a problem if we were trying to skip
    // subtrees. Touch visit_function so the ScopeFlags import isn't
    // dropped if we add helpers later.
    fn visit_function(&mut self, it: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
        walk::walk_function(self, it, flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse_into, source_type_for};
    use crate::source::SourceFile;
    use oxc_allocator::Allocator;
    use std::path::PathBuf;

    #[test]
    fn line_starts_basic() {
        assert_eq!(compute_line_starts(""), vec![0]);
        assert_eq!(compute_line_starts("a"), vec![0]);
        assert_eq!(compute_line_starts("a\nb"), vec![0, 2]);
        assert_eq!(compute_line_starts("a\nb\n"), vec![0, 2, 4]);
    }

    #[test]
    fn byte_to_line_handles_starts() {
        let s = vec![0u32, 5, 10];
        assert_eq!(byte_to_line(&s, 0), 0);
        assert_eq!(byte_to_line(&s, 4), 0);
        assert_eq!(byte_to_line(&s, 5), 1);
        assert_eq!(byte_to_line(&s, 9), 1);
        assert_eq!(byte_to_line(&s, 10), 2);
        assert_eq!(byte_to_line(&s, 99), 2);
    }

    /// Helper to keep the test boilerplate down: returns a closure that
    /// owns the parser allocator + result, and lends LineView slices via
    /// the callback. Avoids the "returns local borrow" lifetime problem.
    fn with_lines(file: SourceFile, body: impl for<'a> FnOnce(&[LineView<'a>])) {
        let alloc = Allocator::default();
        let parsed = parse_into(&alloc, &file);
        let lines: Vec<_> = Lines::build(&file.text, &parsed.program).collect();
        let _st = source_type_for(&file.path); // exercise the helper
        body(&lines);
    }

    // --- cd-cgd: span_for produces file-absolute byte offsets ---------

    #[test]
    fn span_for_uses_file_absolute_bytes() {
        let file = SourceFile::new(
            PathBuf::from("f.ts"),
            "const x = 1;\nconst y = 2;\n".to_string(),
        );
        let text = file.text.clone();
        with_lines(file, |lines| {
            // Line 2 starts at byte 13 (right after the first '\n').
            let line2 = lines[1];
            assert_eq!(line2.line_no, 2);
            assert_eq!(line2.line_start, 13);
            // 'y' is at char offset 6 within "const y = 2;" — file byte 19.
            let span = line2.span_for(6, 7);
            assert_eq!(span.line, 2);
            assert_eq!(span.column, 7);
            assert_eq!(span.start_byte, 19);
            assert_eq!(span.end_byte, 20);
            assert_eq!(&text[span.start_byte as usize..span.end_byte as usize], "y");
        });
    }

    // --- cd-0ne: is_jsx_text flagged for JSXText, not for attributes --

    #[test]
    fn is_jsx_text_flags_text_between_tags() {
        let file = SourceFile::new(
            PathBuf::from("f.tsx"),
            "const el = <div title=\"hi\">Hello world</div>;\n".to_string(),
        );
        with_lines(file, |lines| {
            let l = lines[0];
            assert!(
                l.is_jsx_text,
                "JSX text `Hello world` should set is_jsx_text"
            );
            // The "hi" attribute is a StringLiteral, so is_string_literal also fires.
            assert!(l.is_string_literal);
        });
    }

    #[test]
    fn is_jsx_text_does_not_fire_without_jsx() {
        let file = SourceFile::new(
            PathBuf::from("f.tsx"),
            "const x: string = \"plain\";\n".to_string(),
        );
        with_lines(file, |lines| {
            assert!(!lines[0].is_jsx_text);
            assert!(lines[0].is_string_literal);
        });
    }

    #[test]
    fn is_jsx_text_spans_multiple_lines() {
        // JSXText between <p> and </p> spans the inner lines. The
        // span semantics are *touch-based* — any line a span overlaps
        // gets flagged, including the line containing the opening
        // tag's trailing newline if the JSXText starts there. Plugin
        // checks rely on `is_jsx_text` as a coarse pre-filter and
        // confirm with a regex against `text`, so this is safe and
        // matches the existing string-literal flag's semantics.
        let file = SourceFile::new(
            PathBuf::from("f.tsx"),
            "const el = <p>\nfirst line\nsecond line\n</p>;\n".to_string(),
        );
        with_lines(file, |lines| {
            assert!(lines[1].is_jsx_text); // `first line`
            assert!(lines[2].is_jsx_text); // `second line`
                                           // Line 4 is `</p>;` — past the JSXText, should NOT be flagged.
            assert!(!lines[3].is_jsx_text);
        });
    }
}
