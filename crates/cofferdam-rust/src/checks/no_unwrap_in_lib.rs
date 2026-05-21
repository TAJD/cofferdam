//! `Rust.NoUnwrapInLib` — flags `.unwrap()` / `.expect()` outside test context.
//!
//! Implementation walks the tree-sitter syntax tree looking for
//! `call_expression` nodes whose function is a `field_expression`
//! resolving to `field_identifier` "unwrap" or "expect". For each
//! hit, the check walks up the ancestor chain checking three guards
//! that mark "test context":
//!
//! * `mod tests` — module declaration with the literal name `tests`.
//! * `#[cfg(test)]` — outer attribute carrying `cfg(test)` on the
//!   enclosing function / mod / impl / item.
//! * `#[test]` — outer attribute `test` on the enclosing function.
//!
//! If any guard matches, the call is in test context and the check
//! does not fire. Otherwise it emits one `Issue` per call.
//!
//! No suppression here — the engine's post-collection suppression
//! pass (cofferdam-engine::suppress) handles `// cofferdam-ignore`
//! directives uniformly across all checks.

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Language, Priority, Severity, SourceFile,
};

use crate::parser::{parse_rust, RustParseTree};

pub struct NoUnwrapInLib;

const META: CheckMeta = CheckMeta {
    id: "Rust.NoUnwrapInLib",
    category: Category::Warning,
    base_priority: 12,
    default_severity: Severity::Medium,
    explanation: "Calling `.unwrap()` or `.expect()` in library code panics on `None`/`Err(_)`. Return `Result` and propagate via `?`, or write a `#[test]` for the call.",
    body: include_str!("../../docs/Rust.NoUnwrapInLib.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
};

impl Check for NoUnwrapInLib {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn run(&self, file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // Parse with tree-sitter-rust. ctx.parsed (oxc TS AST) is
        // ignored — this check runs against Rust source. The engine
        // wiring in checkpoint 4 ensures this check is only dispatched
        // on `Language::Rust` files, but defensive None-on-failure is
        // cheap.
        let Ok(tree) = parse_rust(&file.text) else {
            return Vec::new();
        };
        // Skip files with parse errors — emitting `NoUnwrapInLib` on a
        // partial tree risks pointing at recovered-but-wrong nodes.
        // The engine's `Warning.ParseError` channel surfaces the parse
        // failure to the user separately.
        if tree.has_errors() {
            return Vec::new();
        }

        let mut issues = Vec::new();
        let mut cursor = tree.root_node().walk();
        collect_findings(&mut cursor, &tree, file, &mut issues);
        issues
    }
}

fn collect_findings(
    cursor: &mut tree_sitter::TreeCursor<'_>,
    tree: &RustParseTree,
    file: &SourceFile,
    out: &mut Vec<Issue>,
) {
    let node = cursor.node();
    if let Some(method) = unwrap_or_expect_call(node, tree) {
        if !in_test_context(node, tree) {
            let span = tree.span_of(method);
            let method_name = tree.text_of(method);
            out.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "`.{method_name}()` in library code panics on `None`/`Err(_)`; return `Result` and propagate via `?` or move the call into a `#[test]`."
                ),
                file: file.path.clone(),
                span,
                priority: Priority(META.base_priority),
                severity: META.default_severity,
                related: Vec::new(),
            });
        }
    }
    if cursor.goto_first_child() {
        loop {
            collect_findings(cursor, tree, file, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

/// If `node` is a `call_expression` of the form `<receiver>.unwrap()`
/// or `<receiver>.expect(...)`, return the `field_identifier` node so
/// the caller can produce a span pointing at the method name.
fn unwrap_or_expect_call<'a>(
    node: tree_sitter::Node<'a>,
    tree: &RustParseTree,
) -> Option<tree_sitter::Node<'a>> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "field_expression" {
        return None;
    }
    let field = function.child_by_field_name("field")?;
    if field.kind() != "field_identifier" {
        return None;
    }
    let name = tree.text_of(field);
    if name == "unwrap" || name == "expect" {
        Some(field)
    } else {
        None
    }
}

/// Walk ancestors of `node` checking for any of the three test-context
/// markers: `mod tests`, `#[cfg(test)]`, or `#[test]`. Returns `true`
/// when any ancestor satisfies one.
fn in_test_context(node: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    let mut current = node.parent();
    while let Some(ancestor) = current {
        let kind = ancestor.kind();
        // `mod tests { ... }` — the conventional test-module name.
        if kind == "mod_item"
            && ancestor
                .child_by_field_name("name")
                .is_some_and(|name| tree.text_of(name) == "tests")
        {
            return true;
        }
        // `#[test]` only on functions; `#[cfg(test)]` on functions, mods, or impls.
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

/// True when `item` has an outer `#[test]` attribute. In tree-sitter-rust
/// `attribute_item` is a **preceding sibling** of the decorated item, not
/// a child — outer attributes attach by adjacency, not by nesting.
fn item_has_test_attr(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    preceding_attribute_items(item)
        .any(|attr| attribute_macro_name(attr, tree).as_deref() == Some("test"))
}

/// True when `item` has an outer `#[cfg(test)]` attribute (or
/// `#[cfg(any(test, ...))]` / nested forms — the conservative match
/// is "the `test` identifier appears inside a `cfg(...)` token
/// tree"). False positive risk: `#[cfg(not(test))]` would also match,
/// suppressing the check there. Worth refining only if that pattern
/// becomes load-bearing for a real consumer.
fn item_has_cfg_test(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    preceding_attribute_items(item).any(|attr| {
        attribute_macro_name(attr, tree).as_deref() == Some("cfg")
            && attribute_contains_identifier(attr, "test", tree)
    })
}

/// Walk `item`'s preceding siblings collecting `attribute_item` nodes
/// until we hit a non-attribute sibling (a doc comment, a visibility
/// modifier, or another item). Outer attributes in tree-sitter-rust
/// are immediately adjacent to the item they decorate, so the walk
/// stops at the first gap.
fn preceding_attribute_items(
    item: tree_sitter::Node<'_>,
) -> impl Iterator<Item = tree_sitter::Node<'_>> {
    let mut current = item.prev_sibling();
    std::iter::from_fn(move || {
        while let Some(node) = current {
            current = node.prev_sibling();
            match node.kind() {
                "attribute_item" => return Some(node),
                // Line comments / doc comments can appear between an
                // attribute and the item — skip them and keep walking.
                "line_comment" | "block_comment" => continue,
                _ => return None,
            }
        }
        None
    })
}

/// Extract the macro identifier from an `attribute_item` node. For
/// `#[test]` returns `Some("test")`; for `#[cfg(test)]` returns
/// `Some("cfg")`; for `#[derive(Debug)]` returns `Some("derive")`.
///
/// tree-sitter-rust 0.23 doesn't field-name the `attribute_item` →
/// `attribute` edge or the `attribute` → identifier edge; we walk
/// named children directly. (See the `debug_tree_shape` ignored test
/// for the actual sexp shape.)
fn attribute_macro_name(attr_item: tree_sitter::Node<'_>, tree: &RustParseTree) -> Option<String> {
    // Find the `attribute` child. tree-sitter-rust models the shape as
    //   attribute_item ::= "#" "[" attribute "]"
    // where the brackets are anonymous tokens and `attribute` is the
    // single named child.
    //
    // Iterating via `named_children(&mut cursor)` would tie the
    // returned Node's lifetime to the cursor (the tree-sitter Rust
    // binding's iterator lifetime quirk). Use indexed access — clean
    // and the Node's lifetime is the tree's.
    let attribute = find_named_child(attr_item, "attribute")?;
    // The first named child of `attribute` is the macro path:
    // `identifier` for unqualified names (`#[test]`, `#[cfg(...)]`),
    // `scoped_identifier` for qualified ones (`#[foo::bar]`).
    let path = find_named_child_matching(attribute, |k| {
        matches!(k, "identifier" | "scoped_identifier")
    })?;
    Some(tree.text_of(path).to_string())
}

/// Find the first named child with a specific `kind`. Used in lieu of
/// the borrow-checker-hostile `named_children(&mut cursor).find(...)`
/// pattern.
fn find_named_child<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    find_named_child_matching(node, |k| k == kind)
}

fn find_named_child_matching<'a>(
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
fn attribute_contains_identifier(
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::CorpusIndex;
    use std::path::PathBuf;

    fn run_check(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.rs"), src);
        let corpus = CorpusIndex::new();
        let mut ctx = CheckContext::new(&file).with_corpus(&corpus);
        NoUnwrapInLib.run(&file, &mut ctx)
    }

    #[test]
    fn fires_on_unwrap_in_lib_code() {
        let src = include_str!("../../tests/fixtures/no_unwrap_in_lib/flagged_in_lib.rs");
        let issues = run_check(src);
        assert!(
            !issues.is_empty(),
            "expected findings on flagged_in_lib.rs; got: {issues:?}",
        );
        // Every finding should carry the Rust.NoUnwrapInLib id.
        for issue in &issues {
            assert_eq!(issue.check_id, "Rust.NoUnwrapInLib");
        }
    }

    #[test]
    fn silent_inside_cfg_test_module() {
        let src = include_str!("../../tests/fixtures/no_unwrap_in_lib/silent_in_test_module.rs");
        let issues = run_check(src);
        assert!(
            issues.is_empty(),
            "expected zero findings inside `#[cfg(test)] mod tests`; got: {issues:?}",
        );
    }

    #[test]
    fn silent_inside_test_attr_function() {
        let src = include_str!("../../tests/fixtures/no_unwrap_in_lib/silent_with_test_attr.rs");
        let issues = run_check(src);
        assert!(
            issues.is_empty(),
            "expected zero findings inside `#[test] fn`; got: {issues:?}",
        );
    }

    #[test]
    fn fires_only_on_lib_calls_in_mixed_file() {
        let src = include_str!("../../tests/fixtures/no_unwrap_in_lib/mixed.rs");
        let issues = run_check(src);
        // Mixed fixture has 2 lib-context unwraps and 2 test-context
        // ones; expect exactly the lib pair.
        assert_eq!(
            issues.len(),
            2,
            "expected exactly the 2 lib-context findings; got: {issues:?}",
        );
        // Spot-check: both findings land on lines where the lib code
        // calls unwrap/expect (lines 4 and 8 in the fixture).
        let lines: Vec<u32> = issues.iter().map(|i| i.span.line).collect();
        assert_eq!(lines, vec![4, 8], "unexpected lines for lib findings");
    }

    #[test]
    fn skips_files_with_parse_errors() {
        // Malformed input: tree-sitter recovers, but has_errors() == true,
        // so the check declines to emit (avoids false positives on broken
        // syntax).
        let src = "fn () { x.unwrap() }\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }

    #[test]
    fn distinguishes_unwrap_from_unrelated_methods() {
        // Method names that aren't unwrap/expect must not fire.
        let src = "pub fn f(x: String) -> usize { x.len() }\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }
}
