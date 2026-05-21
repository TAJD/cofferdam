//! Shared tree-sitter-rust walking helpers (cd-91zc).
//!
//! Three Rust adapter checks need the same primitives — test-context
//! detection (`#[cfg(test)]` / `#[test]` / `mod tests`), outer-attribute
//! enumeration, and attribute-name extraction. Extracted out of
//! `no_unwrap_in_lib.rs` when the second consumer landed; canonical
//! site for any future Rust check that needs ancestor walking.
//!
//! All helpers operate on `tree_sitter::Node` plus a `&RustParseTree`
//! for text lookups. They take no opinion on which `Issue` to emit —
//! that stays in each per-check file.

use crate::parser::RustParseTree;

/// True when any ancestor of `node` marks "test context" — i.e. the
/// node lives inside `#[cfg(test)] mod tests`, `#[test] fn ...`,
/// `#[cfg(test)] fn ...`, or `mod tests { ... }`. The three guards
/// match Rust's conventional test gating; checks that fire on
/// library-only code (NoUnwrapInLib, NoUnimplementedInNonTest)
/// consult this to skip test code uniformly.
///
/// Conservative match on `cfg(test)` — `#[cfg(not(test))]` also
/// returns true, suppressing checks inside negated-test gates. Worth
/// refining only if a real consumer reports a false negative there.
pub(crate) fn in_test_context(node: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        let kind = ancestor.kind();
        if kind == "mod_item"
            && ancestor
                .child_by_field_name("name")
                .is_some_and(|name| tree.text_of(name) == "tests")
        {
            return true;
        }
        if kind == "function_item" && item_has_test_attr(ancestor, tree) {
            return true;
        }
        if matches!(kind, "function_item" | "mod_item" | "impl_item")
            && item_has_cfg_test(ancestor, tree)
        {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

/// True when `item` has an outer `#[test]` attribute.
pub(crate) fn item_has_test_attr(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    preceding_attribute_items(item)
        .any(|attr| attribute_macro_name(attr, tree).as_deref() == Some("test"))
}

/// True when `item` has an outer `#[cfg(test)]` attribute (or any nested
/// form whose token tree contains the `test` identifier).
pub(crate) fn item_has_cfg_test(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    preceding_attribute_items(item).any(|attr| {
        attribute_macro_name(attr, tree).as_deref() == Some("cfg")
            && attribute_contains_identifier(attr, "test", tree)
    })
}

/// Walk `item`'s preceding siblings collecting `attribute_item` nodes
/// until we hit a non-attribute, non-comment sibling. Outer attributes
/// in tree-sitter-rust are immediately adjacent to the item they
/// decorate — the walk stops at the first gap so attributes belonging
/// to an earlier item don't leak into the result.
pub(crate) fn preceding_attribute_items(
    item: tree_sitter::Node<'_>,
) -> impl Iterator<Item = tree_sitter::Node<'_>> {
    let mut current = item.prev_sibling();
    std::iter::from_fn(move || {
        while let Some(node) = current {
            current = node.prev_sibling();
            match node.kind() {
                "attribute_item" => return Some(node),
                "line_comment" | "block_comment" => continue,
                _ => return None,
            }
        }
        None
    })
}

/// Extract the macro identifier from an `attribute_item` node. For
/// `#[test]` returns `Some("test")`; for `#[cfg(test)]` returns
/// `Some("cfg")`; for `#[doc = "..."]` returns `Some("doc")`.
pub(crate) fn attribute_macro_name(
    attr_item: tree_sitter::Node<'_>,
    tree: &RustParseTree,
) -> Option<String> {
    let attribute = find_named_child(attr_item, "attribute")?;
    let path = find_named_child_matching(attribute, |k| {
        matches!(k, "identifier" | "scoped_identifier")
    })?;
    Some(tree.text_of(path).to_string())
}

/// Find the first named child with a specific `kind`. Borrow-checker-
/// friendly alternative to `named_children(&mut cursor).find(...)`.
pub(crate) fn find_named_child<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    find_named_child_matching(node, |k| k == kind)
}

pub(crate) fn find_named_child_matching<'a>(
    node: tree_sitter::Node<'a>,
    pred: impl Fn(&str) -> bool,
) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.named_child_count() {
        let child = node.named_child(i)?;
        if pred(child.kind()) {
            return Some(child);
        }
    }
    None
}

/// Whether any identifier-shaped descendant of `attr_item` has the
/// literal text `name`. Used by `item_has_cfg_test` to look inside
/// `#[cfg(test)]` / `#[cfg(any(test, ...))]` for the `test` token.
pub(crate) fn attribute_contains_identifier(
    attr_item: tree_sitter::Node<'_>,
    name: &str,
    tree: &RustParseTree,
) -> bool {
    let mut stack = vec![attr_item];
    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" && tree.text_of(node) == name {
            return true;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    false
}
