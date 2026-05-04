//! TypeScript-specific line classification.
//!
//! Walks the parsed comment list + a one-pass literal walk over
//! `StringLiteral`, `TemplateLiteral`, and `JSXText` spans to populate
//! the per-line flag table that drives `cofferdam_core::Lines`.
//!
//! Why a regex isn't enough: `^\s*//` misses block comments split
//! across lines, JSDoc, and pragmas — and a regex over quote
//! characters misclassifies escape sequences and template
//! interpolations. oxc has already paid the parse cost; we lift its
//! comment table and a one-pass literal walk to get accurate flags
//! for free.

use cofferdam_core::{apply_byte_range, compute_line_starts, LineFlags, Lines};
use oxc_ast::ast::{Comment, JSXText, Program, StringLiteral, TemplateLiteral};
use oxc_ast_visit::{walk, Visit};
use oxc_span::Span as OxcSpan;
use oxc_syntax::scope::ScopeFlags;

/// Build a [`Lines`] iterator with TS-flavored classification flags.
///
/// `text` is the file source. `program` is the oxc AST. The returned
/// iterator yields `cofferdam_core::LineView` per line, with flags
/// populated from comments + string/template/JSX-text spans.
pub fn build_lines<'a>(text: &'a str, program: &'a Program<'a>) -> Lines<'a> {
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
        apply_byte_range(&line_starts, &mut flags, sp.start, sp.end, |f| {
            f.is_string_literal = true
        });
    }
    for sp in literal.jsx_text_spans {
        apply_byte_range(&line_starts, &mut flags, sp.start, sp.end, |f| {
            f.is_jsx_text = true
        });
    }

    Lines::from_parts(text, line_starts, flags)
}

fn apply_comment(line_starts: &[u32], flags: &mut [LineFlags], c: &Comment) {
    let is_jsdoc = c.is_jsdoc();
    let is_annotation = c.is_annotation();
    apply_byte_range(line_starts, flags, c.span.start, c.span.end, |f| {
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
    use crate::parser::parse_into;
    use cofferdam_core::SourceFile;
    use oxc_allocator::Allocator;
    use std::path::PathBuf;

    fn with_lines(file: SourceFile, body: impl for<'a> FnOnce(&[cofferdam_core::LineView<'a>])) {
        let alloc = Allocator::default();
        let parsed = parse_into(&alloc, &file);
        let lines: Vec<_> = build_lines(&file.text, &parsed.program).collect();
        body(&lines);
    }

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
        let file = SourceFile::new(
            PathBuf::from("f.tsx"),
            "const el = <p>\nfirst line\nsecond line\n</p>;\n".to_string(),
        );
        with_lines(file, |lines| {
            assert!(lines[1].is_jsx_text);
            assert!(lines[2].is_jsx_text);
            assert!(!lines[3].is_jsx_text);
        });
    }
}
