//! Plugin-facing line view + classification for HTML (CD-84).
//!
//! Mirrors `cofferdam_core::lines::Lines` (the TypeScript line
//! classifier), but the flags are HTML-shaped: `is_tag` / `is_text` /
//! `is_comment` instead of the JS-specific `is_string_literal` /
//! `is_jsx_text`. Built from a walk over the tree-sitter-html parse
//! tree rather than an oxc comment table + literal walk, since HTML has
//! no separate comment/literal pass — every construct is already a
//! syntax node.
//!
//! # Flag semantics
//!
//! Each flag is true when the line is **touched** by the named
//! construct, same "overlap" rule as the TS classifier:
//!
//! - `is_tag` — a `start_tag`, `end_tag`, `self_closing_tag`, or
//!   `doctype` node overlaps this line.
//! - `is_text` — a `text` node overlaps this line.
//! - `is_comment` — a `comment` node overlaps this line.

use crate::parser::HtmlParseTree;

/// One line of HTML source plus classification flags.
#[derive(Debug, Clone, Copy)]
pub struct HtmlLineView<'a> {
    /// 1-based line number.
    pub line_no: u32,
    /// Line text with the trailing `\r` (CRLF) stripped, no `\n`.
    pub text: &'a str,
    pub is_tag: bool,
    pub is_text: bool,
    pub is_comment: bool,
    /// 0-based byte offset of the start of this line in the full source
    /// text.
    pub line_start: u32,
}

/// Build one [`HtmlLineView`] per textual line in `tree`'s source, with
/// classification flags populated from a walk over the parse tree.
pub fn html_lines<'a>(text: &'a str, tree: &HtmlParseTree) -> Vec<HtmlLineView<'a>> {
    let line_starts = compute_line_starts(text);
    let mut flags = vec![LineFlags::default(); line_starts.len().max(1)];

    let mut cursor = tree.root_node().walk();
    collect_flags(&mut cursor, &line_starts, &mut flags);

    let mut out = Vec::with_capacity(line_starts.len());
    for (i, &start) in line_starts.iter().enumerate() {
        let start = start as usize;
        let raw_end = if i + 1 < line_starts.len() {
            (line_starts[i + 1] as usize).saturating_sub(1)
        } else {
            text.len()
        };
        let raw = &text[start..raw_end.min(text.len())];
        let line_text = raw.strip_suffix('\r').unwrap_or(raw);

        let f = flags[i];
        out.push(HtmlLineView {
            line_no: i as u32 + 1,
            text: line_text,
            is_tag: f.is_tag,
            is_text: f.is_text,
            is_comment: f.is_comment,
            line_start: line_starts[i],
        });
    }
    out
}

#[derive(Default, Clone, Copy)]
struct LineFlags {
    is_tag: bool,
    is_text: bool,
    is_comment: bool,
}

fn collect_flags(
    cursor: &mut tree_sitter::TreeCursor<'_>,
    line_starts: &[u32],
    flags: &mut [LineFlags],
) {
    let node = cursor.node();
    match node.kind() {
        "start_tag" | "end_tag" | "self_closing_tag" | "doctype" => {
            apply_span(
                line_starts,
                flags,
                node.start_byte() as u32,
                node.end_byte() as u32,
                |f| {
                    f.is_tag = true;
                },
            );
        }
        "text" => {
            apply_span(
                line_starts,
                flags,
                node.start_byte() as u32,
                node.end_byte() as u32,
                |f| {
                    f.is_text = true;
                },
            );
        }
        "comment" => {
            apply_span(
                line_starts,
                flags,
                node.start_byte() as u32,
                node.end_byte() as u32,
                |f| {
                    f.is_comment = true;
                },
            );
        }
        _ => {}
    }
    if cursor.goto_first_child() {
        loop {
            collect_flags(cursor, line_starts, flags);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn apply_span(
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

fn byte_to_line(line_starts: &[u32], byte: u32) -> u32 {
    match line_starts.binary_search(&byte) {
        Ok(i) => i as u32,
        Err(i) => (i.saturating_sub(1)) as u32,
    }
}

fn compute_line_starts(text: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(text.bytes().filter(|&b| b == b'\n').count() + 1);
    starts.push(0);
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_html;

    #[test]
    fn flags_tag_text_and_comment_lines() {
        let src = "<p>hi</p>\n<!-- note -->\n";
        let tree = parse_html(src).expect("parse");
        let lines = html_lines(src, &tree);
        assert_eq!(lines.len(), 3);

        // Line 1: `<p>hi</p>` — both tag and text overlap.
        assert!(lines[0].is_tag, "start/end tag overlaps line 1");
        assert!(lines[0].is_text, "`hi` text overlaps line 1");
        assert!(!lines[0].is_comment);

        // Line 2: the comment.
        assert!(lines[1].is_comment);
        assert!(!lines[1].is_tag);
        assert!(!lines[1].is_text);
    }

    #[test]
    fn doctype_counts_as_tag() {
        let src = "<!DOCTYPE html>\n<html></html>\n";
        let tree = parse_html(src).expect("parse");
        let lines = html_lines(src, &tree);
        assert!(lines[0].is_tag, "doctype line is flagged as a tag");
        assert!(!lines[0].is_text);
        assert!(!lines[0].is_comment);
    }

    #[test]
    fn line_text_strips_crlf_and_matches_source() {
        let src = "<p>hi</p>\r\n<p>bye</p>\r\n";
        let tree = parse_html(src).expect("parse");
        let lines = html_lines(src, &tree);
        assert_eq!(lines[0].text, "<p>hi</p>");
        assert_eq!(lines[1].text, "<p>bye</p>");
    }
}
