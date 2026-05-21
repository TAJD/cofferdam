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

use crate::parser::RustParseTree;
use crate::tree_walk::in_test_context;

/// `Rust.NoUnwrapInLib` — see the module-level docs for behaviour. The
/// type is `pub` so `all_rust_checks()` can construct it; users do not
/// instantiate this directly.
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

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // cd-0039: the engine parses each `.rs` file once and stashes
        // the tree on `ctx.parsed_lang`; we just downcast. None here
        // means the engine already emitted `Warning.ParseError` for
        // this file — silently skipping mirrors the TS check pattern
        // (`let Some(parsed) = ctx.parsed else { ... };`).
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::CorpusIndex;
    use std::path::PathBuf;

    // Mirrors the engine's per-file dispatch (cd-0039): parse the
    // source once, drop trees with parse errors, install the tree on
    // `parsed_lang`. Tests that pass malformed sources get an empty
    // result the same way the engine path does.
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
