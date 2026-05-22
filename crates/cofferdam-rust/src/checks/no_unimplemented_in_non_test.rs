//! `Rust.NoUnimplementedInNonTest` — flags `unimplemented!()` / `todo!()`
//! calls outside test context.
//!
//! Walks the tree for `macro_invocation` nodes whose macro path is
//! literally `unimplemented` or `todo`. Both expand to a panic; in
//! library code that's a guaranteed runtime crash for any consumer
//! that walks that path. The ancestor-walk gate from
//! `tree_walk::in_test_context` is the same one `NoUnwrapInLib` uses —
//! `#[cfg(test)] mod tests`, `#[test] fn ...`, and `mod tests {}` all
//! suppress the finding.
//!
//! Macro paths are matched against the literal identifier — `core::todo!`
//! and `std::unimplemented!` would not match, on the theory that a fully
//! qualified path is rare and almost always deliberate. If a real
//! consumer reports a false negative on a qualified form, widen the
//! match to `scoped_identifier` last-segment.

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Language, Location, Priority, Severity,
    SourceFile,
};

use crate::parser::RustParseTree;
use crate::tree_walk::in_test_context;

/// `Rust.NoUnimplementedInNonTest` — see the module-level docs for
/// behaviour. The type is `pub` so `all_rust_checks()` can construct
/// it; users do not instantiate this directly.
pub struct NoUnimplementedInNonTest;

const META: CheckMeta = CheckMeta {
    id: "Rust.NoUnimplementedInNonTest",
    category: Category::Warning,
    base_priority: 14,
    default_severity: Severity::High,
    explanation: "`unimplemented!()` / `todo!()` panic at runtime; calling them outside test code ships a guaranteed crash. Implement the function or move it into a `#[test]`.",
    body: include_str!("../../docs/Rust.NoUnimplementedInNonTest.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for NoUnimplementedInNonTest {
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
    if let Some((macro_name_node, macro_name)) = panicking_macro(node, tree) {
        if !in_test_context(node, tree) {
            let span = tree.span_of(macro_name_node);
            out.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "`{macro_name}!()` panics at runtime; implement the function or move the call into a `#[test]`."
                ),
                file: file.path.clone(),
                location: Location::from_span(&file.path, span),
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

/// If `node` is a `macro_invocation` whose path is `unimplemented` or
/// `todo`, return the identifier node (for span) and the macro name.
fn panicking_macro<'a>(
    node: tree_sitter::Node<'a>,
    tree: &RustParseTree,
) -> Option<(tree_sitter::Node<'a>, &'static str)> {
    if node.kind() != "macro_invocation" {
        return None;
    }
    // The macro path is the first named child; in unqualified form
    // it's an `identifier`. Qualified forms (`std::todo`) produce
    // `scoped_identifier` and are deliberately not matched here.
    let path = node.named_child(0)?;
    if path.kind() != "identifier" {
        return None;
    }
    let name = tree.text_of(path);
    let canonical = match name {
        "unimplemented" => "unimplemented",
        "todo" => "todo",
        _ => return None,
    };
    Some((path, canonical))
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
        NoUnimplementedInNonTest.run(&file, &mut ctx)
    }

    #[test]
    fn fires_on_unimplemented_in_lib() {
        let issues = run_check("pub fn f() -> i32 { unimplemented!() }\n");
        assert_eq!(issues.len(), 1, "got: {issues:?}");
        assert_eq!(issues[0].check_id, "Rust.NoUnimplementedInNonTest");
    }

    #[test]
    fn fires_on_todo_in_lib() {
        let issues = run_check("pub fn f() -> i32 { todo!() }\n");
        assert_eq!(issues.len(), 1, "got: {issues:?}");
    }

    #[test]
    fn silent_inside_cfg_test_module() {
        let src = "#[cfg(test)]\nmod tests {\n    fn helper() { todo!() }\n}\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_inside_test_attr_function() {
        let src = "#[test]\nfn t() { unimplemented!() }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn silent_inside_named_tests_module() {
        let src = "mod tests {\n    fn helper() { todo!() }\n}\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn ignores_other_macros() {
        // `println!` / `format!` / `assert!` etc. must not fire — only
        // the runtime-panic macros.
        let src = "pub fn f() { println!(\"x\"); assert!(true); }\n";
        let issues = run_check(src);
        assert!(issues.is_empty(), "got: {issues:?}");
    }

    #[test]
    fn skips_files_with_parse_errors() {
        // Malformed input: tree-sitter recovers, but `has_errors()` is
        // true, so the check declines.
        let src = "fn () { todo!() }\n";
        let issues = run_check(src);
        assert!(issues.is_empty());
    }
}
