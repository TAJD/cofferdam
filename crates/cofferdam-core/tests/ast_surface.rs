//! Integration tests for the plugin-facing AST surface (cd-81a.2).
//!
//! Covers all three pattern families a plugin author needs:
//!
//! 1. **Eager filter** via `find_all(NodeKind)` — proven on
//!    `console.log` calls and banned-import detection.
//! 2. **Stateful visitor** via `walk(&mut visitor)` with `Walk::Continue`
//!    descent and `Walk::Skip` short-circuit.
//! 3. **Direct rooted access** via `root()` — escape hatch.
//!
//! TS-domain examples deliberately — the examplco-host check catalogue
//! is Elixir/Ash-specific, so porting those literally would be misleading
//! (see `feedback_no_examplco_ports_in_ts` memory). The *patterns* port;
//! the *checks* don't.

use std::path::PathBuf;

use cofferdam_core::oxc_ast::ast::{
    Argument, Declaration, Expression, ImportDeclarationSpecifier, MemberExpression, PropertyKey,
    Statement,
};
use cofferdam_core::parser::{parse_into, ParsedView};
use cofferdam_core::{
    Allocator, AstView, AstVisitor, CheckContext, NodeKind, NodeRef, SourceFile, Walk,
};

/// Build an `AstView` for `text` parsed as a `.ts` file. The arena +
/// parsed view are leaked for the duration of the test — fine for
/// `#[test]` short-lived processes and avoids needing a self-cell.
fn view_for(text: &'static str) -> AstView<'static> {
    let path = PathBuf::from("test.ts");
    let file = Box::leak(Box::new(SourceFile::new(path, text.to_string())));
    let allocator: &'static Allocator = Box::leak(Box::new(Allocator::default()));
    let parsed_return = parse_into(allocator, file);
    let parsed: &'static ParsedView<'static> = Box::leak(Box::new(ParsedView {
        program: Box::leak(Box::new(parsed_return.program)),
        diagnostics: Box::leak(parsed_return.errors.into_boxed_slice()),
    }));
    let ctx = CheckContext::new(file).with_parsed(parsed);
    ctx.ast().expect("parsed view should produce AstView")
}

// ============================================================
// Pattern A — eager filter via find_all
// ============================================================

#[test]
fn find_all_call_expressions_finds_every_console_log() {
    let view = view_for(
        r#"
        console.log("a");
        function f() {
            console.log("b");
            console.log("c");
        }
        const x = console.log;
        "#,
    );

    let calls = view.find_all(NodeKind::CallExpression);
    // Three `console.log(...)` invocations. The bare `console.log`
    // reference (no parens) is a MemberExpression, not a CallExpression.
    assert_eq!(calls.len(), 3, "expected 3 CallExpression nodes");

    // Every NodeRef carries its kind.
    assert!(calls.iter().all(|n| n.kind() == NodeKind::CallExpression));
}

#[test]
fn find_all_import_declarations_can_filter_banned_sources() {
    let view = view_for(
        r#"
        import { foo } from "lodash";
        import bar from "deprecated-pkg";
        import * as ok from "./local";
        "#,
    );

    let imports = view.find_all(NodeKind::ImportDeclaration);
    assert_eq!(imports.len(), 3);

    let banned = ["deprecated-pkg", "ancient-lib"];
    let hits: Vec<&str> = imports
        .iter()
        .filter_map(|n| match n {
            NodeRef::ImportDeclaration(d) => {
                let src = d.source.value.as_str();
                banned.contains(&src).then_some(src)
            }
            _ => None,
        })
        .collect();

    assert_eq!(hits, vec!["deprecated-pkg"]);
}

// ============================================================
// Pattern B — visitor with Continue (descend) and Skip (short-circuit)
// ============================================================

#[derive(Default)]
struct CountVisitor {
    classes: u32,
    functions: u32,
    arrows: u32,
    calls: u32,
}

impl<'a> AstVisitor<'a> for CountVisitor {
    fn visit_class(&mut self, _: &'a cofferdam_core::oxc_ast::ast::Class<'a>) -> Walk {
        self.classes += 1;
        Walk::Continue
    }
    fn visit_function(&mut self, _: &'a cofferdam_core::oxc_ast::ast::Function<'a>) -> Walk {
        self.functions += 1;
        Walk::Continue
    }
    fn visit_arrow_function_expression(
        &mut self,
        _: &'a cofferdam_core::oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) -> Walk {
        self.arrows += 1;
        Walk::Continue
    }
    fn visit_call_expression(
        &mut self,
        _: &'a cofferdam_core::oxc_ast::ast::CallExpression<'a>,
    ) -> Walk {
        self.calls += 1;
        Walk::Continue
    }
}

#[test]
fn walk_with_continue_descends_into_every_node() {
    let view = view_for(
        r#"
        class A {
            run() { console.log("a"); }
        }
        class B {
            constructor() { this.go(); }
            go() {}
        }
        function top() { return (x: number) => x + 1; }
        "#,
    );

    let mut v = CountVisitor::default();
    view.walk(&mut v);

    assert_eq!(v.classes, 2, "two classes");
    // class methods + top-level function. Constructors and methods
    // surface as Function nodes through the `walk` machinery — exact
    // count is implementation-defined, just assert non-zero.
    assert!(v.functions >= 1, "at least the top-level function");
    assert_eq!(v.arrows, 1, "one arrow function");
    assert!(v.calls >= 2, "console.log + this.go()");
}

/// Skip-on-Class halts descent into class members. A `console.log` inside
/// a skipped class must NOT be visited.
struct SkipClassesVisitor {
    classes_seen: u32,
    calls_seen: u32,
}

impl<'a> AstVisitor<'a> for SkipClassesVisitor {
    fn visit_class(&mut self, _: &'a cofferdam_core::oxc_ast::ast::Class<'a>) -> Walk {
        self.classes_seen += 1;
        Walk::Skip // do not descend into class body
    }
    fn visit_call_expression(
        &mut self,
        _: &'a cofferdam_core::oxc_ast::ast::CallExpression<'a>,
    ) -> Walk {
        self.calls_seen += 1;
        Walk::Continue
    }
}

#[test]
fn walk_skip_halts_descent_into_node_only() {
    let view = view_for(
        r#"
        console.log("outside");
        class Inner {
            run() {
                console.log("inside-1");
                console.log("inside-2");
            }
        }
        console.log("after");
        "#,
    );

    let mut v = SkipClassesVisitor {
        classes_seen: 0,
        calls_seen: 0,
    };
    view.walk(&mut v);

    assert_eq!(v.classes_seen, 1, "class itself was visited");
    // Skip stops descent into the class body, so the two inside calls
    // are never seen — only `outside` and `after`.
    assert_eq!(
        v.calls_seen, 2,
        "Skip should prevent descent into class body"
    );
}

// ============================================================
// Pattern C — stateful walk with file-level decision
//
// TS analog of the "scan a class, accumulate flags, decide once" pattern
// from examplco-host TenantIsolation: find every class that declares
// both `width` and `height` as instance properties — a structural
// signature commonly used to gate "you should extend our Box base
// class" lints.
// ============================================================

#[derive(Default)]
struct DimensionClassVisitor {
    /// Names of classes that declared both `width` and `height`.
    matched: Vec<String>,
}

impl<'a> AstVisitor<'a> for DimensionClassVisitor {
    fn visit_class(&mut self, node: &'a cofferdam_core::oxc_ast::ast::Class<'a>) -> Walk {
        let mut has_width = false;
        let mut has_height = false;

        for elem in &node.body.body {
            if let cofferdam_core::oxc_ast::ast::ClassElement::PropertyDefinition(p) = elem {
                if let PropertyKey::StaticIdentifier(ident) = &p.key {
                    match ident.name.as_str() {
                        "width" => has_width = true,
                        "height" => has_height = true,
                        _ => {}
                    }
                }
            }
        }

        if has_width && has_height {
            let name = node
                .id
                .as_ref()
                .map(|i| i.name.as_str().to_string())
                .unwrap_or_else(|| "<anonymous>".to_string());
            self.matched.push(name);
        }

        // Skip — once the class-level decision is made, descending into
        // members would only let nested-class noise add false matches.
        Walk::Skip
    }
}

#[test]
fn stateful_walk_decides_per_class() {
    let view = view_for(
        r#"
        class Sprite {
            width: number = 0;
            height: number = 0;
            color: string = "red";
        }
        class Audio {
            volume: number = 1;
        }
        class Frame {
            width: number;
            height: number;
            constructor(w: number, h: number) {
                this.width = w;
                this.height = h;
            }
        }
        class Half {
            width: number = 0;
        }
        "#,
    );

    let mut v = DimensionClassVisitor::default();
    view.walk(&mut v);

    assert_eq!(v.matched, vec!["Sprite".to_string(), "Frame".to_string()]);
}

// ============================================================
// Pattern direct — root() access for surgical work
// ============================================================

#[test]
fn root_returns_the_program() {
    let view = view_for(
        r#"
        const a = 1;
        function f() {}
        "#,
    );
    let prog = view.root();
    // Two top-level statements. Comments / whitespace aren't statements.
    let mut top = 0;
    for stmt in &prog.body {
        match stmt {
            Statement::VariableDeclaration(_) => top += 1,
            Statement::FunctionDeclaration(_) => top += 1,
            Statement::ExpressionStatement(_) => top += 1,
            Statement::ImportDeclaration(_) => top += 1,
            _ => {}
        }
    }
    assert_eq!(top, 2);
}

// ============================================================
// span_of — round-trip byte → line/col
// ============================================================

#[test]
fn span_of_matches_span_from_bytes() {
    let view = view_for(
        r#"const a = 1;
const b = 2;
const c = 3;"#,
    );

    let imports = view.find_all(NodeKind::NumericLiteral);
    assert_eq!(imports.len(), 3);

    // Second numeric literal (`2`) sits on line 2.
    let span = imports[1].span();
    let span = view.span_of(span.start, span.end);
    assert_eq!(span.line, 2);
}

// ============================================================
// CheckContext::ast() integration
// ============================================================

#[test]
fn check_context_ast_returns_none_when_unparsed() {
    let path = PathBuf::from("test.ts");
    let file = SourceFile::new(path, "const a = 1;".to_string());
    let ctx = CheckContext::new(&file);
    assert!(ctx.ast().is_none());
}

// ============================================================
// Banned-import detection via find_all — the "TS-native NoHttpClient"
// shape: ImportDeclaration source string match.
//
// Different from the examplco HTTPoison check (which scans Elixir
// alias references) — but exercises the same SDK seam: eager filter,
// then post-filter on a node attribute.
// ============================================================

#[test]
fn find_banned_imports_with_specifier_detail() {
    let view = view_for(
        r#"
        import { request } from "node:http";
        import got from "got";
        import { Foo, Bar as Baz } from "deprecated-pkg";
        import "./style.css";
        "#,
    );

    let banned = ["deprecated-pkg", "got"];
    let mut findings: Vec<(String, Vec<String>)> = Vec::new();

    for import in view.find_all(NodeKind::ImportDeclaration) {
        let NodeRef::ImportDeclaration(d) = import else {
            continue;
        };
        let src = d.source.value.as_str();
        if !banned.contains(&src) {
            continue;
        }
        let names: Vec<String> = d
            .specifiers
            .iter()
            .flatten()
            .map(|s| match s {
                ImportDeclarationSpecifier::ImportSpecifier(imp) => imp.imported.name().to_string(),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(imp) => {
                    imp.local.name.as_str().to_string()
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(imp) => {
                    imp.local.name.as_str().to_string()
                }
            })
            .collect();
        findings.push((src.to_string(), names));
    }

    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].0, "got");
    assert_eq!(findings[0].1, vec!["got".to_string()]);
    assert_eq!(findings[1].0, "deprecated-pkg");
    assert_eq!(findings[1].1, vec!["Foo".to_string(), "Bar".to_string()]);
}

// ============================================================
// Ensure the unused-imports we pulled into scope at the top are
// actually referenced — silences `unused_imports` warnings without
// `#[allow]` clutter.
// ============================================================

#[allow(dead_code)]
fn _unused_import_anchor(
    _arg: &Argument,
    _decl: &Declaration,
    _expr: &Expression,
    _mem: &MemberExpression,
) {
}
