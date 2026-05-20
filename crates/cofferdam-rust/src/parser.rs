//! tree-sitter-rust → cofferdam `Span` conversion layer (cd-91zc checkpoint 2).
//!
//! Parses Rust source into a borrowing handle and exposes the two
//! conversions every downstream check needs:
//!
//! * `RustParseTree::span_of(node)` — produces a cofferdam `Span` from
//!   a tree-sitter node's byte range, using the same `span_from_bytes`
//!   helper the TypeScript adapter uses. Spans round-trip exactly:
//!   slicing the source by `(start_byte, end_byte)` yields the node's
//!   textual extent. Multi-byte UTF-8 identifiers inflate the column
//!   count proportionally — same behaviour as the TS adapter, will
//!   become grapheme-aware in `span_util`'s phase-2 rewrite.
//!
//! * `RustParseTree::text_of(node)` — returns the borrowed source slice
//!   for the node. Useful for per-node pattern matching (e.g. matching
//!   `unwrap` as the method name on a call expression).
//!
//! tree-sitter is error-recovery-oriented: parsing always produces a
//! tree, even when the input is malformed. The recovery happens by
//! inserting `ERROR` nodes; downstream checks call
//! [`RustParseTree::has_errors`] (or iterate `error_spans()`) to decide
//! whether to trust the tree or skip the file.

use std::fmt;

use cofferdam_core::{span_from_bytes, Span};

/// Errors that can prevent `parse_rust` from returning a tree.
#[derive(Debug)]
pub enum RustParseError {
    /// Loading the tree-sitter-rust grammar into the parser failed.
    /// In practice this only fires when the linked tree-sitter and
    /// tree-sitter-rust crates have an ABI mismatch — bump them together.
    LanguageLoad(String),
    /// `tree_sitter::Parser::parse` returned `None`. The official
    /// reasons (per upstream docs): the input parser was cancelled
    /// via a `ParseOptions::progress_callback`, or the timeout
    /// elapsed. We don't set either; in normal use this branch is
    /// unreachable.
    NoTree,
}

impl fmt::Display for RustParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LanguageLoad(msg) => {
                write!(f, "failed to load tree-sitter-rust grammar: {msg}")
            }
            Self::NoTree => write!(
                f,
                "tree-sitter returned no tree (parser cancelled or timed out)"
            ),
        }
    }
}

impl std::error::Error for RustParseError {}

/// A parsed Rust source. Owns the source text so downstream check
/// authors can slice arbitrary node ranges without lifetime gymnastics.
///
/// Construct via [`parse_rust`].
pub struct RustParseTree {
    tree: tree_sitter::Tree,
    text: String,
}

impl RustParseTree {
    /// The top-level `source_file` node. Walk this with
    /// `tree_sitter::TreeCursor` for visitor-style traversals, or use
    /// `Node::child(i)` / `Node::children(&mut cursor)` for direct
    /// access.
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    /// The borrowed source text. Same string `parse_rust` received.
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
    tree: &RustParseTree,
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

/// Parse a Rust source string into a [`RustParseTree`].
///
/// The parser is constructed per call; tree-sitter::Parser is cheap to
/// build (microseconds) and not `Send`/`Sync`. Engine-level reuse
/// across many files lands in cd-91zc checkpoint 4 alongside the
/// per-language dispatch.
pub fn parse_rust(text: &str) -> Result<RustParseTree, RustParseError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| RustParseError::LanguageLoad(e.to_string()))?;
    let tree = parser.parse(text, None).ok_or(RustParseError::NoTree)?;
    Ok(RustParseTree {
        tree,
        text: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trivial-source smoke test promoted from `lib.rs`'s
    /// `tree_sitter_rust_parses_trivial_source` to live alongside the
    /// rest of the parser tests. The smoke test in `lib.rs` stays as
    /// a crate-level link check.
    #[test]
    fn parses_trivial_source() {
        let tree = parse_rust("fn main() {}").expect("parse");
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!tree.has_errors());
        assert!(root.child_count() > 0);
    }

    /// `span_of` produces byte ranges that slice back to the node's
    /// textual extent. This is the round-trip cd-9hp.12 / cd-svf-style
    /// guardrail: byte arithmetic in the conversion layer is the
    /// fastest place to introduce off-by-one bugs.
    #[test]
    fn span_of_function_item_round_trips() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        let tree = parse_rust(src).expect("parse");
        let func = find_first(&tree, "function_item").expect("function_item present");
        let span = tree.span_of(func);
        // Span's byte range slices back to the same text.
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, tree.text_of(func));
        // The function starts at line 1, column 1.
        assert_eq!(span.line, 1);
        assert_eq!(span.column, 1);
    }

    /// UTF-8 sanity: identifiers with multi-byte chars don't break the
    /// byte arithmetic. The column count tracks bytes (matching the
    /// TS adapter); grapheme-aware columns are a forward-compat
    /// concern tracked in `cofferdam_core::span_util`.
    #[test]
    fn utf8_identifier_round_trips() {
        let src = "fn café() {}\n";
        let tree = parse_rust(src).expect("parse");
        let func = find_first(&tree, "function_item").expect("function_item present");
        let span = tree.span_of(func);
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, "fn café() {}");
        assert_eq!(tree.text_of(func), "fn café() {}");
        assert!(!tree.has_errors());
    }

    /// Multi-line source: line/column for a node nested inside a
    /// nested `mod` reflects the actual source position, not just
    /// the byte offset.
    #[test]
    fn multiline_nested_modules_track_lines() {
        let src = "mod outer {\n    mod inner {\n        fn deep() {}\n    }\n}\n";
        let tree = parse_rust(src).expect("parse");
        let func = find_first(&tree, "function_item").expect("function_item present");
        let span = tree.span_of(func);
        // The `fn deep() {}` lives on line 3 (1-based), indented 8 columns.
        assert_eq!(span.line, 3);
        assert_eq!(span.column, 9); // 1-based column after 8 spaces of indent
                                    // Bytes still round-trip.
        let slice = &src[span.start_byte as usize..span.end_byte as usize];
        assert_eq!(slice, "fn deep() {}");
    }

    /// Malformed input still produces a tree (tree-sitter is
    /// recovery-oriented) but `has_errors` returns `true` and
    /// `error_spans` reports at least one location.
    #[test]
    fn has_errors_on_malformed_input() {
        let src = "fn () {}\n";
        let tree = parse_rust(src).expect("parse");
        assert!(
            tree.has_errors(),
            "expected has_errors=true for malformed source `fn () {{}}`",
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
        let src = "fn main() { let x = 1; }\n";
        let tree = parse_rust(src).expect("parse");
        assert!(!tree.has_errors());
        assert!(tree.error_spans().is_empty());
    }

    /// `text_of` returns the borrowed slice corresponding to a node's
    /// byte range. Sanity check that node ranges and source slices
    /// agree across a few node kinds.
    #[test]
    fn text_of_returns_node_source_slice() {
        let src = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
        let tree = parse_rust(src).expect("parse");
        let func = find_first(&tree, "function_item").expect("function_item present");
        assert_eq!(
            tree.text_of(func),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }"
        );
        let ident = find_first_named(&tree, "identifier").expect("identifier present");
        assert_eq!(tree.text_of(ident), "add");
    }

    // ─── helpers ──────────────────────────────────────────────────────────

    /// Find the first node in document order whose `kind()` matches.
    /// Test-only convenience.
    fn find_first<'tree>(
        tree: &'tree RustParseTree,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        let mut cursor = tree.root_node().walk();
        walk_find(&mut cursor, |n| n.kind() == kind)
    }

    /// Find the first *named* node (skips anonymous syntactic tokens)
    /// whose `kind()` matches. tree-sitter classifies `(`, `)`, `;`, etc.
    /// as anonymous; `identifier`, `function_item`, `block` are named.
    fn find_first_named<'tree>(
        tree: &'tree RustParseTree,
        kind: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        let mut cursor = tree.root_node().walk();
        walk_find(&mut cursor, |n| n.is_named() && n.kind() == kind)
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
