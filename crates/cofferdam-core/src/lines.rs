//! Line view + line-table mechanics — the **data shape** of per-line
//! classification. The actual classification (string-literal walk,
//! JSX-text walk, comment-table mapping) is language-specific and lives
//! in language-adapter crates such as `cofferdam-ts`.
//!
//! Why this split (cd-8wj): every text-based language has lines and
//! shares the byte-offset → line/column nuance; only the *flag-setting*
//! pass differs. Keeping the struct + line-table here means a future
//! Python or Go adapter only re-implements the flag-population step,
//! not the iterator plumbing.
//!
//! # Flag semantics
//!
//! Each flag is true when the line is **touched** by the named
//! construct — a multi-line block comment marks every line it spans;
//! a template literal marks every line of the backtick-enclosed text
//! including embedded `${expr}` lines. The *meaning* is fixed by this
//! struct; the *populator* is owned by the language adapter and may
//! choose to leave a flag permanently `false` if the language has no
//! corresponding construct.
//!
//! - `is_comment` — any comment overlaps this line.
//! - `is_doc_comment` — a doc-style comment (e.g. JSDoc `/** ... */`)
//!   overlaps. Implies `is_comment`.
//! - `is_string_literal` — a string-literal or template-literal span
//!   overlaps this line.
//! - `is_jsx_text` — a JSXText span overlaps this line. Adapter-specific
//!   (TS only); other languages always leave this `false`.
//! - `is_pragma` — an annotation-style comment overlaps. For TS that
//!   maps to `oxc_ast::Comment::is_annotation()` — pragmas are
//!   compiler-hint comments like `/* #__PURE__ */`, `/* @vite-ignore */`,
//!   `/* webpackChunkName: "x" */` — *not* JSDoc and *not* legal
//!   headers.

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

/// Iterator over [`LineView`]s. Constructed by language adapters via
/// [`Lines::from_parts`] after they've populated per-line flags.
pub struct Lines<'a> {
    text: &'a str,
    /// Byte offset of the start of each line. `len() == number of lines`.
    line_starts: Vec<u32>,
    flags: Vec<LineFlags>,
    idx: usize,
}

impl<'a> Lines<'a> {
    /// Build `Lines` from a fully-populated flag table. Adapter-facing:
    /// language adapters compute `line_starts` (typically via
    /// [`compute_line_starts`]) and `flags` (via their own classification
    /// pass), then hand both back here for iteration.
    pub fn from_parts(text: &'a str, line_starts: Vec<u32>, flags: Vec<LineFlags>) -> Self {
        debug_assert_eq!(
            line_starts.len().max(1),
            flags.len(),
            "flag table must match line count"
        );
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

/// Per-line flag table populated by language adapters. Adapter walks
/// the parsed comment list + string/JSX/template-literal spans, then
/// hands the table back to [`Lines::from_parts`].
///
/// Public so adapters in sibling crates can construct + mutate it; the
/// fields stay `pub` rather than going through setters because every
/// adapter wants the same flat write pattern (`flags[i].is_comment = true`).
#[derive(Default, Clone, Copy)]
pub struct LineFlags {
    pub is_comment: bool,
    pub is_doc_comment: bool,
    pub is_string_literal: bool,
    pub is_jsx_text: bool,
    pub is_pragma: bool,
}

/// Indices into the source where each line starts. Always begins with 0;
/// subsequent entries point one byte past every `\n`.
pub fn compute_line_starts(text: &str) -> Vec<u32> {
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
pub fn byte_to_line(line_starts: &[u32], byte: u32) -> u32 {
    match line_starts.binary_search(&byte) {
        Ok(i) => i as u32,
        // Err(i) means byte fits between starts[i-1] and starts[i].
        // Saturate so a byte before line 1 (impossible in practice)
        // still maps to line 0.
        Err(i) => (i.saturating_sub(1)) as u32,
    }
}

/// Apply `set` to every line that the byte range `[start, end)` overlaps.
/// Adapter-facing helper for the classification pass — every adapter
/// applies its own per-construct flag, but the line-overlap iteration
/// is identical across languages.
///
/// `start..end` is the *exclusive* convention; an empty span (start == end)
/// is treated as touching `start`'s line only.
pub fn apply_byte_range(
    line_starts: &[u32],
    flags: &mut [LineFlags],
    start: u32,
    end: u32,
    mut set: impl FnMut(&mut LineFlags),
) {
    let start_line = byte_to_line(line_starts, start);
    let last_byte = end.saturating_sub(1).max(start);
    let end_line = byte_to_line(line_starts, last_byte);
    for li in start_line..=end_line {
        if let Some(f) = flags.get_mut(li as usize) {
            set(f);
        }
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

    #[test]
    fn span_for_uses_file_absolute_bytes() {
        let text = "const x = 1;\nconst y = 2;\n";
        let line_starts = compute_line_starts(text);
        // Empty flag table — we're testing the iterator + span_for, not
        // classification.
        let flags = vec![LineFlags::default(); line_starts.len()];
        let lines: Vec<_> = Lines::from_parts(text, line_starts, flags).collect();
        let line2 = lines[1];
        assert_eq!(line2.line_no, 2);
        assert_eq!(line2.line_start, 13);
        let span = line2.span_for(6, 7);
        assert_eq!(span.line, 2);
        assert_eq!(span.column, 7);
        assert_eq!(span.start_byte, 19);
        assert_eq!(span.end_byte, 20);
        assert_eq!(&text[span.start_byte as usize..span.end_byte as usize], "y");
    }
}
