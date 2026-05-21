//! `Rust.MissingPubDoc` — flags public items without an outer doc
//! comment (or `#[doc = "..."]` attribute).
//!
//! Targets the four item kinds that compose a published Rust API
//! surface — `pub fn`, `pub struct`, `pub enum`, `pub trait`. Anything
//! with a restricted visibility (`pub(crate)`, `pub(super)`,
//! `pub(in path)`) is exempt: it isn't part of the published surface,
//! so missing docs there is internal-API ergonomics, not a public
//! contract gap.
//!
//! What counts as documented:
//!
//! * a preceding `/// ...` line doc comment,
//! * a preceding `/** ... */` block doc comment,
//! * a preceding `#[doc = "..."]` attribute,
//! * or any preceding `#[doc(hidden)]` attribute (deliberately
//!   undocumented — clippy's `missing_docs` honours the same opt-out).
//!
//! Items inside test context (`#[cfg(test)]`, `mod tests`, `#[test]`)
//! are silent for the same reason as the other Rust checks — test
//! support code isn't part of the published surface.

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Language, Priority, Severity, SourceFile,
};

use crate::parser::RustParseTree;
use crate::tree_walk::{
    attribute_contains_identifier, attribute_macro_name, in_test_context, preceding_attribute_items,
};

/// `Rust.MissingPubDoc` — see the module-level docs for behaviour. The
/// type is `pub` so `all_rust_checks()` can construct it; users do not
/// instantiate this directly.
pub struct MissingPubDoc;

const META: CheckMeta = CheckMeta {
    id: "Rust.MissingPubDoc",
    category: Category::Design,
    base_priority: 4,
    default_severity: Severity::Low,
    explanation: "Public items in a library crate compose the published API surface. Document each `pub fn` / `pub struct` / `pub enum` / `pub trait` with a `///` doc comment so consumers can understand what to call.",
    body: include_str!("../../docs/Rust.MissingPubDoc.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for MissingPubDoc {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn language(&self) -> Language {
        Language::Rust
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(tree) = ctx.parsed_as::<RustParseTree>() else {
            return Vec::new();
        };
        let mut issues = Vec::new();
        let mut cursor = tree.root_node().walk();
        collect_findings(&mut cursor, tree, file, &mut issues);
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
    if let Some((name_node, kind_label)) = candidate_pub_item(node, tree) {
        if !in_test_context(node, tree)
            && !has_doc_marker(node, tree)
            && !has_doc_hidden(node, tree)
        {
            let span = tree.span_of(name_node);
            let item_name = tree.text_of(name_node);
            out.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "public {kind_label} `{item_name}` is missing a `///` doc comment."
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

/// If `node` is a `pub`-with-no-restriction `fn` / `struct` / `enum` /
/// `trait` item, return its name node (for the span) and a label for
/// the diagnostic ("function", "struct", etc.).
fn candidate_pub_item<'a>(
    node: tree_sitter::Node<'a>,
    tree: &RustParseTree,
) -> Option<(tree_sitter::Node<'a>, &'static str)> {
    let kind_label = match node.kind() {
        "function_item" => "function",
        "struct_item" => "struct",
        "enum_item" => "enum",
        "trait_item" => "trait",
        _ => return None,
    };
    if !has_unrestricted_pub(node, tree) {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    Some((name, kind_label))
}

/// True when the item's visibility modifier is exactly `pub` — not
/// `pub(crate)`, `pub(super)`, `pub(in ...)`, and not absent.
fn has_unrestricted_pub(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    for i in 0..item.named_child_count() {
        let Some(child) = item.named_child(i) else {
            return false;
        };
        if child.kind() == "visibility_modifier" {
            return tree.text_of(child).trim() == "pub";
        }
    }
    false
}

/// True when the item has any outer documentation: `///` line doc,
/// `/** */` block doc, or `#[doc = "..."]` attribute.
fn has_doc_marker(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    let mut current = item.prev_sibling();
    while let Some(sibling) = current {
        current = sibling.prev_sibling();
        match sibling.kind() {
            "line_comment" => {
                let text = tree.text_of(sibling);
                // `////` is an overstrike (visual divider), not a doc
                // comment. `///` and `//!` are outer/inner doc lines.
                // For an item's prev_sibling only outer (`///`) counts.
                if text.starts_with("///") && !text.starts_with("////") {
                    return true;
                }
                // Plain `// ...` non-doc comment — keep walking.
                continue;
            }
            "block_comment" => {
                let text = tree.text_of(sibling);
                if text.starts_with("/**") && !text.starts_with("/***") && text != "/**/" {
                    return true;
                }
                continue;
            }
            "attribute_item" => {
                if attribute_macro_name(sibling, tree).as_deref() == Some("doc") {
                    return true;
                }
                // Non-doc attribute (e.g. `#[inline]`) — keep walking.
                continue;
            }
            _ => return false,
        }
    }
    false
}

/// True when any preceding attribute is `#[doc(hidden)]`.
fn has_doc_hidden(item: tree_sitter::Node<'_>, tree: &RustParseTree) -> bool {
    preceding_attribute_items(item).any(|attr| {
        attribute_macro_name(attr, tree).as_deref() == Some("doc")
            && attribute_contains_identifier(attr, "hidden", tree)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::CorpusIndex;
    use std::path::PathBuf;

    // Mirrors engine dispatch (cd-0039).
    fn run_check(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.rs"), src);
        let corpus = CorpusIndex::new();
        let tree = match crate::parser::parse_rust(&file.text) {
            Ok(t) if !t.has_errors() => t,
            _ => return Vec::new(),
        };
        let mut ctx = CheckContext::new(&file)
            .with_corpus(&corpus)
            .with_parsed_lang(&tree);
        MissingPubDoc.run(&file, &mut ctx)
    }

    #[test]
    fn fires_on_undocumented_pub_fn() {
        let issues = run_check("pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("function `add`"));
    }

    #[test]
    fn fires_on_undocumented_pub_struct() {
        let issues = run_check("pub struct Foo { x: i32 }\n");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("struct `Foo`"));
    }

    #[test]
    fn fires_on_undocumented_pub_enum() {
        let issues = run_check("pub enum E { A, B }\n");
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn fires_on_undocumented_pub_trait() {
        let issues = run_check("pub trait Foo { fn bar(&self); }\n");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("trait `Foo`"));
    }

    #[test]
    fn silent_with_line_doc_comment() {
        let src = "/// Adds two ints.\npub fn add(a: i32, b: i32) -> i32 { a + b }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_with_block_doc_comment() {
        let src = "/** Adds two ints. */\npub fn add(a: i32, b: i32) -> i32 { a + b }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_with_doc_attribute() {
        let src = "#[doc = \"Adds two ints.\"]\npub fn add(a: i32, b: i32) -> i32 { a + b }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_with_doc_hidden() {
        let src = "#[doc(hidden)]\npub fn internal() {}\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn doc_through_intervening_attribute() {
        // Doc comment must be reachable through an intervening
        // non-doc attribute (e.g. `#[inline]`).
        let src = "/// Adds two ints.\n#[inline]\npub fn add() {}\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn ignores_private_items() {
        // No visibility modifier → not part of the public surface.
        let src = "fn helper() {}\nstruct Local;\nenum E { A }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn ignores_pub_crate_items() {
        // Restricted visibility is internal API.
        let src = "pub(crate) fn helper() {}\npub(crate) struct Internal;\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_in_test_context() {
        let src = "#[cfg(test)]\nmod tests {\n    pub fn helper() {}\n}\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn overstrike_line_does_not_count_as_doc() {
        // `//// foo` is an overstrike comment (visual divider). It does
        // NOT satisfy the doc requirement — clippy treats it the same.
        let src = "//// section divider\npub fn add() {}\n";
        let issues = run_check(src);
        assert_eq!(issues.len(), 1, "got: {issues:?}");
    }

    #[test]
    fn fires_on_multiple_items_in_one_file() {
        let src = "pub fn a() {}\npub fn b() {}\npub struct C;\n";
        let issues = run_check(src);
        assert_eq!(issues.len(), 3, "got: {issues:?}");
    }

    #[test]
    fn skips_files_with_parse_errors() {
        let src = "pub fn () {}\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }
}
