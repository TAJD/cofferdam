//! tree-sitter-html → cofferdam `Span` conversion layer (CD-83).
//!
//! Parses HTML source into a borrowing handle and exposes the same two
//! conversions the Rust adapter's parser exposes:
//!
//! * `HtmlParseTree::span_of(node)` — produces a cofferdam `Span` from
//!   a tree-sitter node's byte range, using the same `span_from_bytes`
//!   helper the TypeScript and Rust adapters use. Spans round-trip
//!   exactly: slicing the source by `(start_byte, end_byte)` yields the
//!   node's textual extent.
//!
//! * `HtmlParseTree::text_of(node)` — returns the borrowed source slice
//!   for the node.
//!
//! tree-sitter is error-recovery-oriented: parsing always produces a
//! tree, even when the input is malformed. Downstream checks call
//! [`HtmlParseTree::has_errors`] (or iterate `error_spans()`) to decide
//! whether to trust the tree or skip the file.

use std::fmt;

use cofferdam_core::{span_from_bytes, Span};

/// Errors that can prevent `parse_html` from returning a tree.
#[derive(Debug)]
pub enum HtmlParseError {
    /// Loading the tree-sitter-html grammar into the parser failed.
    /// In practice this only fires when the linked tree-sitter and
    /// tree-sitter-html crates have an ABI mismatch — bump them together.
    LanguageLoad(String),
    /// `tree_sitter::Parser::parse` returned `None`. The official
    /// reasons (per upstream docs): the input parser was cancelled
    /// via a `ParseOptions::progress_callback`, or the timeout
    /// elapsed. We don't set either; in normal use this branch is
    /// unreachable.
    NoTree,
}

impl fmt::Display for HtmlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LanguageLoad(msg) => {
                write!(f, "failed to load tree-sitter-html grammar: {msg}")
            }
            Self::NoTree => write!(
                f,
                "tree-sitter returned no tree (parser cancelled or timed out)"
            ),
        }
    }
}

impl std::error::Error for HtmlParseError {}

/// A parsed HTML source. Owns the source text so downstream check
/// authors can slice arbitrary node ranges without lifetime gymnastics.
///
/// Construct via [`parse_html`].
pub struct HtmlParseTree {
    tree: tree_sitter::Tree,
    text: String,
}

impl HtmlParseTree {
    /// The top-level `document` node. Walk this with
    /// `tree_sitter::TreeCursor` for visitor-style traversals, or use
    /// `Node::child(i)` / `Node::children(&mut cursor)` for direct
    /// access.
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    /// The borrowed source text. Same string `parse_html` received.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Convert a tree-sitter node's byte range into a cofferdam `Span`
    /// (start/end bytes plus 1-based line/column).
    pub fn span_of(&self, node: tree_sitter::Node<'_>) -> Span {
        span_from_bytes(&self.text, node.start_byte() as u32, node.end_byte() as u32)
    }

    /// The source slice corresponding to `node`'s byte range. Returns
    /// `""` if the node's range is somehow out of bounds (should not
    /// happen for nodes from this tree; the bounds check is defensive).
    pub fn text_of(&self, node: tree_sitter::Node<'_>) -> &str {
        let start = node.start_byte();
        let end = node.end_byte();
        self.text.get(start..end).unwrap_or("")
    }

    /// `true` when any node in the tree (including descendants) is an
    /// `ERROR` node or has a missing child. Checks that emit findings
    /// on partial parses risk false positives — most should skip when
    /// this returns `true`.
    pub fn has_errors(&self) -> bool {
        self.root_node().has_error()
    }

    /// Every `ERROR` and `MISSING` node in the tree, as cofferdam
    /// `Span`s. Useful for surfacing a `Warning.ParseError`-shaped
    /// finding to the user, or for skipping affected regions while
    /// continuing to analyse the rest of the file.
    pub fn error_spans(&self) -> Vec<Span> {
        let mut out = Vec::new();
        let mut cursor = self.tree.walk();
        collect_errors(&mut cursor, self, &mut out);
        out
    }
}

fn collect_errors(
    cursor: &mut tree_sitter::TreeCursor<'_>,
    tree: &HtmlParseTree,
    out: &mut Vec<Span>,
) {
    let node = cursor.node();
    if node.is_error() || node.is_missing() {
        out.push(tree.span_of(node));
    }
    if cursor.goto_first_child() {
        loop {
            collect_errors(cursor, tree, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// Parse an HTML source string into an [`HtmlParseTree`].
///
/// The parser is constructed per call; tree-sitter::Parser is cheap to
/// build (microseconds) and not `Send`/`Sync`.
pub fn parse_html(text: &str) -> Result<HtmlParseTree, HtmlParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_html::LANGUAGE.into())
        .map_err(|e| HtmlParseError::LanguageLoad(e.to_string()))?;
    let tree = parser.parse(text, None).ok_or(HtmlParseError::NoTree)?;
    Ok(HtmlParseTree {
        tree,
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trivial_source() {
        let tree = parse_html("<html><head></head><body></body></html>").expect("parse");
        let root = tree.root_node();
        assert_eq!(root.kind(), "document");
        assert!(!tree.has_errors());
        assert!(root.child_count() > 0);
    }

    /// `span_of` produces byte ranges that slice back to the node's
    /// textual extent. Round-trip guardrail mirroring the Rust
    /// adapter's `span_of_function_item_round_trips`.
    #[test]
    fn span_of_element_round_trips() {
        let src = "<html>\n<body>\n<p>hi</p>\n</body>\n</html>\n";
        let tree = parse_html(src).expect("parse");
        let p = find_by_tag(&tree, "p").expect("p element present");
        let span = tree.span_of(p);
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, tree.text_of(p));
        assert_eq!(slice, "<p>hi</p>");
    }

    /// Multi-byte content: a paragraph containing non-ASCII text
    /// doesn't break the byte arithmetic.
    #[test]
    fn multibyte_content_round_trips() {
        let src = "<html><body><p>café ☕</p></body></html>\n";
        let tree = parse_html(src).expect("parse");
        let root = tree.root_node();
        let span = tree.span_of(root);
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, src);
        assert!(!tree.has_errors());
    }

    /// Multi-line source: line/column for a nested node reflects the
    /// actual source position, not just the byte offset.
    #[test]
    fn multiline_nested_elements_track_lines() {
        let src = "<html>\n<body>\n<div>\n<p>deep</p>\n</div>\n</body>\n</html>\n";
        let tree = parse_html(src).expect("parse");
        // The innermost `<p>` lives on line 4 (1-based), column 1.
        let p = find_by_tag(&tree, "p").expect("p element present");
        let span = tree.span_of(p);
        assert_eq!(span.line, 4);
        assert_eq!(span.column, 1);
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, "<p>deep</p>");
    }

    /// Malformed input: a stray `<` with no closing structure produces
    /// an `ERROR` node. tree-sitter-html is lenient about unclosed tags
    /// (it treats them as implicitly closed per HTML5 parsing rules),
    /// so an unclosed `<div>` alone does NOT reliably produce an ERROR
    /// node — a genuinely broken fragment (bare `<` followed by
    /// non-tag text) does.
    #[test]
    fn has_errors_on_malformed_input() {
        let src = "<html><body>< oops</body></html>\n";
        let tree = parse_html(src).expect("parse");
        assert!(
            tree.has_errors(),
            "expected has_errors=true for malformed source `< oops`",
        );
        let errors = tree.error_spans();
        assert!(
            !errors.is_empty(),
            "expected at least one error span; got {errors:?}",
        );
    }

    /// Clean input produces zero error spans.
    #[test]
    fn no_errors_on_clean_input() {
        let src = "<html><head><title>t</title></head><body><p>ok</p></body></html>\n";
        let tree = parse_html(src).expect("parse");
        assert!(!tree.has_errors());
        assert!(tree.error_spans().is_empty());
    }

    /// `text_of` returns the borrowed slice corresponding to a node's
    /// byte range.
    #[test]
    fn text_of_returns_node_source_slice() {
        let src = "<html><body><p>hello</p></body></html>\n";
        let tree = parse_html(src).expect("parse");
        let p = find_by_tag(&tree, "p").expect("p element present");
        assert_eq!(tree.text_of(p), "<p>hello</p>");
    }

    // ─── helpers ──────────────────────────────────────────────────────────

    /// Find the first `element` node whose start tag's tag name equals
    /// `tag`. Test-only convenience for picking a specific element out
    /// of a nested document. tree-sitter-html's grammar doesn't expose
    /// field names for `tag_name` / `attribute_name` (unlike
    /// tree-sitter-rust's `function_item.name`), so lookups here and in
    /// `checks::missing_lang_attribute` walk children by `kind()`.
    fn find_by_tag<'tree>(
        tree: &'tree HtmlParseTree,
        tag: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        let mut cursor = tree.root_node().walk();
        walk_find(&mut cursor, |n| {
            if n.kind() != "element" {
                return false;
            }
            let Some(start_tag) = n.child(0) else {
                return false;
            };
            if start_tag.kind() != "start_tag" {
                return false;
            }
            let Some(name) = first_child_of_kind(start_tag, "tag_name") else {
                return false;
            };
            tree.text_of(name) == tag
        })
    }

    fn first_child_of_kind<'tree>(
        node: tree_sitter::Node<'tree>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == kind {
                    return Some(child);
                }
            }
        }
        None
    }

    fn walk_find<'tree>(
        cursor: &mut tree_sitter::TreeCursor<'tree>,
        pred: impl Fn(tree_sitter::Node<'tree>) -> bool + Copy,
    ) -> Option<tree_sitter::Node<'tree>> {
        let node = cursor.node();
        if pred(node) {
            return Some(node);
        }
        if cursor.goto_first_child() {
            loop {
                if let Some(found) = walk_find(cursor, pred) {
                    return Some(found);
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
        None
    }
}
