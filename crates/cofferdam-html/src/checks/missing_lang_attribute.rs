//! `Html.MissingLangAttribute` — flags an `<html>` element with no
//! `lang` attribute.
//!
//! The `lang` attribute tells browsers, screen readers, and search
//! engines which language the page's text is in. Without it,
//! assistive technology falls back to guessing (or the user's
//! default), and search engines can't serve the page to the right
//! locale — both an accessibility and an SEO concern (ties into the
//! CD-79 epic this check belongs to).
//!
//! Implementation walks the tree-sitter syntax tree for the document's
//! `<html>` element and checks its start tag's attribute list for
//! `lang` (case-insensitive, matching HTML attribute-name semantics).
//! An empty `lang=""` still counts as present — this check only flags
//! the attribute's absence, not a bad value.

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Language, Location, Priority, Severity,
    SourceFile,
};

use crate::parser::HtmlParseTree;

/// `Html.MissingLangAttribute` — see the module-level docs for
/// behaviour. The type is `pub` so `all_html_checks()` can construct
/// it; users do not instantiate this directly.
pub struct MissingLangAttribute;

const META: CheckMeta = CheckMeta {
    id: "Html.MissingLangAttribute",
    category: Category::Warning,
    base_priority: 12,
    default_severity: Severity::Medium,
    explanation: "The document's `<html>` element has no `lang` attribute, so assistive technology and search engines can't determine the page's language.",
    body: include_str!("../../docs/Html.MissingLangAttribute.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for MissingLangAttribute {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn languages(&self) -> &'static [Language] {
        &[Language::Html]
    }

    /// A build-output HTML page missing `lang` is exactly the kind of
    /// finding `cofferdam verify --dist` should catch (CD-85).
    fn output_mode(&self) -> bool {
        true
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // cd-0039-style dispatch: the engine parses each `.html` file
        // once and stashes the tree on `ctx.parsed_lang`; we just
        // downcast. `None` here means the engine already emitted
        // `Warning.ParseError` for this file.
        let Some(tree) = ctx.parsed_as::<HtmlParseTree>() else {
            return Vec::new();
        };

        let Some(html_element) = find_html_element(tree) else {
            return Vec::new();
        };
        let Some(start_tag) = start_tag_of(html_element) else {
            return Vec::new();
        };
        if has_lang_attribute(start_tag, tree) {
            return Vec::new();
        }

        let span = tree.span_of(start_tag);
        vec![Issue {
            check_id: META.id.to_string(),
            message: "<html> element has no `lang` attribute; add one (e.g. `lang=\"en\"`) \
                so assistive technology and search engines know the page's language."
                .to_string(),
            file: file.path.clone(),
            location: Location::from_span(&file.path, span),
            priority: Priority(META.base_priority),
            severity: META.default_severity,
            related: Vec::new(),
        }]
    }
}

/// Find the document's `<html>` element by walking the tree looking
/// for an `element` node whose start tag's name is `html`
/// (case-insensitive).
fn find_html_element<'a>(tree: &'a HtmlParseTree) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = tree.root_node().walk();
    walk_find(&mut cursor, |n| {
        if n.kind() != "element" {
            return false;
        }
        let Some(start_tag) = start_tag_of(n) else {
            return false;
        };
        tag_name(start_tag, tree).is_some_and(|name| name.eq_ignore_ascii_case("html"))
    })
}

/// The `start_tag` child of an `element` node, or `None` for void /
/// self-closing elements that have no separate start tag node.
fn start_tag_of(element: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    first_child_of_kind(element, "start_tag")
}

/// Find the first direct child of `node` whose `kind()` matches.
fn first_child_of_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return Some(child);
            }
        }
    }
    None
}

/// The tag name text of a `start_tag` node (e.g. `"html"`).
///
/// tree-sitter-html doesn't expose a `name` field on `start_tag` (unlike
/// tree-sitter-rust's `function_item.name`), so this looks up the first
/// `tag_name` child by kind instead.
fn tag_name(start_tag: tree_sitter::Node<'_>, tree: &HtmlParseTree) -> Option<String> {
    first_child_of_kind(start_tag, "tag_name").map(|n| tree.text_of(n).to_string())
}

/// True when `start_tag` has an attribute named `lang`
/// (case-insensitive), regardless of its value (including `lang=""`).
fn has_lang_attribute(start_tag: tree_sitter::Node<'_>, tree: &HtmlParseTree) -> bool {
    for i in 0..start_tag.child_count() {
        let Some(child) = start_tag.child(i) else {
            continue;
        };
        if child.kind() != "attribute" {
            continue;
        }
        let Some(name_node) = first_child_of_kind(child, "attribute_name") else {
            continue;
        };
        if tree.text_of(name_node).eq_ignore_ascii_case("lang") {
            return true;
        }
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::CorpusIndex;
    use std::path::PathBuf;

    // Mirrors the engine's per-file dispatch: parse the source once,
    // drop trees with parse errors, install the tree on `parsed_lang`.
    fn run_check(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.html"), src);
        let corpus = CorpusIndex::new();
        let tree = match crate::parser::parse_html(&file.text) {
            Ok(t) if !t.has_errors() => t,
            _ => return Vec::new(),
        };
        let mut ctx = CheckContext::new(&file)
            .with_corpus(&corpus)
            .with_parsed_lang(&tree);
        MissingLangAttribute.run(&file, &mut ctx)
    }

    #[test]
    fn output_mode_is_enabled() {
        assert!(MissingLangAttribute.output_mode());
    }

    #[test]
    fn fires_when_lang_attribute_absent() {
        let src = "<html><head><title>t</title></head><body></body></html>\n";
        let issues = run_check(src);
        assert_eq!(
            issues.len(),
            1,
            "expected exactly 1 finding; got: {issues:?}"
        );
        assert_eq!(issues[0].check_id, "Html.MissingLangAttribute");
    }

    #[test]
    fn silent_when_lang_attribute_present() {
        let src = "<html lang=\"en\"><head><title>t</title></head><body></body></html>\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "expected zero findings; got: {issues:?}");
    }

    #[test]
    fn silent_when_lang_attribute_empty() {
        // Presence, not value, is what this check enforces.
        let src = "<html lang=\"\"><body></body></html>\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "expected zero findings; got: {issues:?}");
    }

    #[test]
    fn lang_attribute_match_is_case_insensitive() {
        let src = "<html LANG=\"en\"><body></body></html>\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "expected zero findings; got: {issues:?}");
    }

    #[test]
    fn skips_files_with_parse_errors() {
        let src = "<html><body>< oops</body></html>\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }

    #[test]
    fn skips_documents_without_html_element() {
        let src = "<p>fragment with no html element</p>\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }
}
