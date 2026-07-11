//! HTML AST wire builder (CD-84).
//!
//! Builds the same `AstWire`/`WireNode`/`WireExtras` shape `ast_wire.rs`
//! produces for TS/JSX, but by walking a `cofferdam_html::HtmlParseTree`
//! (tree-sitter-html) instead of an oxc `Program`. Lives in
//! `cofferdam-cli` rather than `cofferdam-html` because `cofferdam-cli`
//! already depends on `cofferdam-html` (not the reverse) — putting the
//! wire builder here avoids a dependency cycle and keeps `ast_wire.rs`'s
//! `WireNode`/`WireExtras` types as the single wire-format source of
//! truth for both source languages.
//!
//! Emits five wire kinds: `Document` (synthetic root, mirrors
//! `Program`), `Element`, `Attribute`, `Text`, `Comment`, `Doctype`.
//! `Element`/`Attribute` reuse `WireExtras::Element`/`WireExtras::Attribute`
//! verbatim — the JSX wire builder's shape (tagName/selfClosing/
//! attributeIdxs, name/value/isExpression/isSpread) already matches what
//! HTML needs; `isExpression`/`isSpread` are always `false` for HTML
//! attributes (HTML has no JSX-style expression or spread attributes).
//!
//! tree-sitter-html node kinds walked here (verified via
//! `Node::to_sexp()` on sample input — the grammar doesn't expose field
//! names, same caveat as `checks::missing_lang_attribute`):
//! `document`, `doctype`, `element`, `start_tag`/`self_closing_tag`
//! (wrapping `tag_name` + `attribute` children), `end_tag`, `attribute`
//! (wrapping `attribute_name` + `attribute_value` or
//! `quoted_attribute_value`), `text`, `comment`, `entity` (skipped — no
//! v0 wire representation, part of surrounding text).

use cofferdam_html::HtmlParseTree;

use crate::ast_wire::{AstWire, WireExtras, WireNode};

struct StackFrame {
    parent_idx: i32,
    last_child_idx: i32,
}

pub struct HtmlWireBuilder<'a> {
    tree: &'a HtmlParseTree,
    nodes: Vec<WireNode>,
    stack: Vec<StackFrame>,
}

impl<'a> HtmlWireBuilder<'a> {
    pub fn new(tree: &'a HtmlParseTree) -> Self {
        Self {
            tree,
            nodes: Vec::new(),
            stack: Vec::new(),
        }
    }

    pub fn build(mut self) -> AstWire {
        let root = self.tree.root_node();
        let idx = self.push("Document", root);
        self.enter(idx);
        self.visit_children(root);
        self.exit();

        let root_idx = if self.nodes.is_empty() { -1 } else { 0 };
        AstWire {
            root_idx,
            nodes: self.nodes,
        }
    }

    fn push(&mut self, kind: &'static str, node: tree_sitter::Node<'_>) -> i32 {
        let idx = self.nodes.len() as i32;
        let (parent_idx, prev_sibling) = match self.stack.last() {
            Some(f) => (f.parent_idx, f.last_child_idx),
            None => (-1, -1),
        };

        self.nodes.push(WireNode {
            kind,
            span: self.tree.span_of(node),
            first_child: -1,
            next_sibling: -1,
            extras: WireExtras::None {},
        });

        if prev_sibling >= 0 {
            self.nodes[prev_sibling as usize].next_sibling = idx;
        } else if parent_idx >= 0 {
            self.nodes[parent_idx as usize].first_child = idx;
        }

        if let Some(f) = self.stack.last_mut() {
            f.last_child_idx = idx;
        }
        idx
    }

    fn enter(&mut self, idx: i32) {
        self.stack.push(StackFrame {
            parent_idx: idx,
            last_child_idx: -1,
        });
    }

    fn exit(&mut self) {
        self.stack.pop();
    }

    /// Visit every direct child of `node`, dispatching each by kind.
    fn visit_children(&mut self, node: tree_sitter::Node<'_>) {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.visit_node(child);
            }
        }
    }

    fn visit_node(&mut self, node: tree_sitter::Node<'_>) {
        match node.kind() {
            "doctype" => {
                let text = self.tree.text_of(node).to_string();
                let idx = self.push("Doctype", node);
                self.nodes[idx as usize].extras = WireExtras::Doctype { text };
            }
            "element" => self.visit_element(node),
            "text" => {
                let text = self.tree.text_of(node).to_string();
                let idx = self.push("Text", node);
                self.nodes[idx as usize].extras = WireExtras::Text { text };
            }
            "comment" => {
                let text = self.tree.text_of(node).to_string();
                let idx = self.push("Comment", node);
                self.nodes[idx as usize].extras = WireExtras::Comment { text };
            }
            // `entity` (e.g. `&amp;`) has no v0 wire representation —
            // it's part of the surrounding text content, same treatment
            // as a non-v0 oxc node in `ast_wire.rs`.
            "entity" => {}
            // Encountered only via `document`/error-recovery nesting;
            // `element` visits its own start/end tag children directly
            // (see `visit_element`), so a bare `start_tag`/`end_tag`
            // here would be a stray token — nothing useful to emit.
            "start_tag" | "end_tag" | "self_closing_tag" | "attribute" => {}
            // ERROR/MISSING nodes from malformed input: descend so any
            // well-formed content nested inside is still discoverable,
            // matching the parser's error-recovery philosophy (a
            // partial tree is still useful) documented in
            // `HtmlParseTree::has_errors`.
            _ => self.visit_children(node),
        }
    }

    fn visit_element(&mut self, node: tree_sitter::Node<'_>) {
        let mut tag_node = None;
        let mut self_closing = false;
        for i in 0..node.child_count() {
            if let Some(c) = node.child(i) {
                match c.kind() {
                    "start_tag" => {
                        tag_node = Some(c);
                        break;
                    }
                    "self_closing_tag" => {
                        tag_node = Some(c);
                        self_closing = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        // No start/self-closing tag (e.g. a raw text node misparsed as
        // an element in a malformed document) — fall back to visiting
        // children directly rather than emitting a tag-less Element.
        let Some(tag_node) = tag_node else {
            self.visit_children(node);
            return;
        };

        let tag_name = first_child_of_kind(tag_node, "tag_name")
            .map(|n| self.tree.text_of(n).to_string())
            .unwrap_or_default();

        let idx = self.push("Element", node);
        self.enter(idx);

        let mut attribute_idxs = Vec::new();
        for i in 0..tag_node.child_count() {
            if let Some(c) = tag_node.child(i) {
                if c.kind() == "attribute" {
                    attribute_idxs.push(self.push_attribute(c));
                }
            }
        }

        for i in 0..node.child_count() {
            if let Some(c) = node.child(i) {
                match c.kind() {
                    "start_tag" | "end_tag" | "self_closing_tag" => {}
                    _ => self.visit_node(c),
                }
            }
        }

        self.exit();
        self.nodes[idx as usize].extras = WireExtras::Element {
            tag_name,
            self_closing,
            attribute_idxs,
        };
    }

    fn push_attribute(&mut self, node: tree_sitter::Node<'_>) -> i32 {
        let name =
            first_child_of_kind(node, "attribute_name").map(|n| self.tree.text_of(n).to_string());
        let value = attribute_value_text(node, self.tree);
        let idx = self.push("Attribute", node);
        self.nodes[idx as usize].extras = WireExtras::Attribute {
            name,
            value,
            is_expression: false,
            is_spread: false,
        };
        idx
    }
}

/// The attribute's value text, or `None` for a boolean attribute
/// (`disabled`) with no `=value` at all. `alt=""` (empty quoted value)
/// returns `Some("")`, distinct from the boolean-attribute `None` case.
fn attribute_value_text(attr: tree_sitter::Node<'_>, tree: &HtmlParseTree) -> Option<String> {
    for i in 0..attr.child_count() {
        if let Some(c) = attr.child(i) {
            match c.kind() {
                "quoted_attribute_value" => {
                    return Some(
                        first_child_of_kind(c, "attribute_value")
                            .map(|v| tree.text_of(v).to_string())
                            .unwrap_or_default(),
                    );
                }
                "attribute_value" => return Some(tree.text_of(c).to_string()),
                _ => {}
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_html::parse_html;

    fn build_wire(src: &str) -> AstWire {
        let tree = parse_html(src).expect("parse");
        HtmlWireBuilder::new(&tree).build()
    }

    fn element_extras(wire: &AstWire, idx: i32) -> (&str, bool, &[i32]) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::Element {
                tag_name,
                self_closing,
                attribute_idxs,
            } => (tag_name, *self_closing, attribute_idxs),
            other => panic!(
                "node {idx} is not an Element: {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    fn attribute_extras(wire: &AstWire, idx: i32) -> (Option<&str>, Option<&str>) {
        match &wire.nodes[idx as usize].extras {
            WireExtras::Attribute { name, value, .. } => (name.as_deref(), value.as_deref()),
            other => panic!(
                "node {idx} is not an Attribute: {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn document_root_has_doctype_and_html_element_children() {
        let wire = build_wire("<!DOCTYPE html>\n<html><body></body></html>\n");
        assert_eq!(wire.root_idx, 0);
        assert_eq!(wire.nodes[0].kind, "Document");

        let doctype = wire
            .nodes
            .iter()
            .find(|n| n.kind == "Doctype")
            .expect("Doctype node emitted");
        match &doctype.extras {
            WireExtras::Doctype { text } => assert_eq!(text, "<!DOCTYPE html>"),
            other => panic!("wrong extras: {:?}", std::mem::discriminant(other)),
        }

        assert!(
            wire.nodes.iter().any(|n| n.kind == "Element"),
            "html element must be emitted"
        );
    }

    #[test]
    fn element_extracts_tag_name_and_self_closing() {
        let wire = build_wire(r#"<img src="cat.png" alt="a cat">"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "Element")
            .expect("Element must be emitted") as i32;
        let (tag_name, self_closing, attribute_idxs) = element_extras(&wire, el_idx);
        assert_eq!(tag_name, "img");
        assert!(!self_closing, "<img ...> with no `/>` is not self-closing");
        assert_eq!(attribute_idxs.len(), 2);

        let (src_name, src_value) = attribute_extras(&wire, attribute_idxs[0]);
        assert_eq!(src_name, Some("src"));
        assert_eq!(src_value, Some("cat.png"));

        let (alt_name, alt_value) = attribute_extras(&wire, attribute_idxs[1]);
        assert_eq!(alt_name, Some("alt"));
        assert_eq!(alt_value, Some("a cat"));
    }

    #[test]
    fn self_closing_element_is_flagged() {
        let wire = build_wire(r#"<img src="cat.png" />"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "Element")
            .expect("Element must be emitted") as i32;
        let (_, self_closing, _) = element_extras(&wire, el_idx);
        assert!(self_closing);
    }

    #[test]
    fn missing_attribute_value_is_none_but_empty_value_is_some_empty() {
        let wire = build_wire(r#"<img src="cat.png" alt="" disabled>"#);
        let el_idx = wire
            .nodes
            .iter()
            .position(|n| n.kind == "Element")
            .expect("Element must be emitted") as i32;
        let (_, _, attribute_idxs) = element_extras(&wire, el_idx);
        assert_eq!(attribute_idxs.len(), 3);

        let (_, alt_value) = attribute_extras(&wire, attribute_idxs[1]);
        assert_eq!(alt_value, Some(""), "alt=\"\" is present but empty");

        let (disabled_name, disabled_value) = attribute_extras(&wire, attribute_idxs[2]);
        assert_eq!(disabled_name, Some("disabled"));
        assert_eq!(disabled_value, None, "boolean attribute has no value");
    }

    #[test]
    fn title_element_text_content_is_discoverable() {
        let wire = build_wire("<html><head><title>My Page</title></head></html>");
        let title_idx = wire.nodes.iter().position(|n| {
            n.kind == "Element"
                && matches!(&n.extras, WireExtras::Element { tag_name, .. } if tag_name == "title")
        });
        let title_idx = title_idx.expect("title element must be emitted") as i32;
        let text_idx = wire.nodes[title_idx as usize].first_child;
        assert!(text_idx >= 0, "title element has a text child");
        match &wire.nodes[text_idx as usize].extras {
            WireExtras::Text { text } => assert_eq!(text, "My Page"),
            other => panic!("wrong extras: {:?}", std::mem::discriminant(other)),
        }
    }

    #[test]
    fn comment_node_carries_raw_text() {
        let wire = build_wire("<!-- a note -->\n<p>hi</p>");
        let comment = wire
            .nodes
            .iter()
            .find(|n| n.kind == "Comment")
            .expect("Comment node emitted");
        match &comment.extras {
            WireExtras::Comment { text } => assert_eq!(text, "<!-- a note -->"),
            other => panic!("wrong extras: {:?}", std::mem::discriminant(other)),
        }
    }

    #[test]
    fn round_trips_byte_spans() {
        let src = "<p>hello</p>\n";
        let wire = build_wire(src);
        let el = wire
            .nodes
            .iter()
            .find(|n| n.kind == "Element")
            .expect("Element must be emitted");
        let slice = &src[el.span.start_byte as usize..el.span.end_byte as usize];
        assert_eq!(slice, "<p>hello</p>");
    }
}
