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
//! - `is_pragma` — an annotation-style comment overlaps. Pragmas are
//!   compiler-hint comments like `/* #__PURE__ */`, `/* @vite-ignore */`,
//!   `/* webpackChunkName: "x" */` — *not* JSDoc and *not* legal
//!   headers. Maps to `oxc_ast::Comment::is_annotation()`.

use oxc_ast::ast::{Comment, StringLiteral, TemplateLiteral};
use oxc_ast_visit::{walk, Visit};
use oxc_span::Span as OxcSpan;
use oxc_syntax::scope::ScopeFlags;

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
    pub is_pragma: bool,
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

        let mut literal = LiteralCollector { spans: Vec::new() };
        literal.visit_program(program);
        for sp in literal.spans {
            apply_span(&line_starts, &mut flags, sp, |f| f.is_string_literal = true);
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
            is_pragma: f.is_pragma,
        })
    }
}

#[derive(Default, Clone, Copy)]
struct LineFlags {
    is_comment: bool,
    is_doc_comment: bool,
    is_string_literal: bool,
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

// ---- internal: AST walk for string + template literal spans --------

struct LiteralCollector {
    spans: Vec<OxcSpan>,
}

impl<'a> Visit<'a> for LiteralCollector {
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        self.spans.push(it.span);
    }
    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        self.spans.push(it.span);
        // Descend so nested templates inside `${...}` interpolations
        // are still recorded.
        walk::walk_template_literal(self, it);
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
}
