use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    Argument, BindingPattern, Expression, ExpressionStatement, ForOfStatement, ForStatement,
    Statement, VariableDeclarationKind,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

/// `Refactor.PreferArrayMethodOverLoop` — flag a `for`/`for-of` loop whose
/// entire body is exactly a computed value pushed onto a single
/// accumulator array (optionally gated by one `if` with no `else`),
/// suggesting `.map()`/`.filter()` instead.
///
/// Deliberately narrow to avoid false positives on loops with early
/// `break`/`continue`, multiple accumulators, or side effects beyond the
/// single push: the body (or the `if`'s consequent) must match one of
/// exactly two shapes, nothing else —
///   1. a single `arr.push(<expr>)` statement, or
///   2. a `const`/`let` declaration with an initializer, immediately
///      followed by `arr.push(<thatName>)`.
///
/// Any other statement in the body (an extra assignment, a `break`, a
/// second push, a nested loop) disqualifies the match entirely, since
/// the shapes above require an exact statement-list match.
pub struct PreferArrayMethodOverLoop;

const META: CheckMeta = CheckMeta {
    id: "Refactor.PreferArrayMethodOverLoop",
    category: Category::Refactor,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A loop whose entire body pushes one computed value (optionally gated by a \
        single `if`) onto an accumulator array is more clearly expressed as `.map()`/`.filter()`.",
    body: include_str!("../../docs/Refactor.PreferArrayMethodOverLoop.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for PreferArrayMethodOverLoop {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = Collector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

/// Flatten a loop/if body into its statement list: a `BlockStatement`
/// yields its own statements, a bare single statement (`for (...) x;`)
/// yields itself as a one-element list.
fn body_statements<'a, 'b>(body: &'b Statement<'a>) -> Vec<&'b Statement<'a>> {
    match body {
        Statement::BlockStatement(block) => block.body.iter().collect(),
        other => vec![other],
    }
}

/// If `expr_stmt` is exactly `<ident>.push(<one arg>)`, return the
/// accumulator's name and the pushed argument.
fn extract_push<'a, 'b>(
    expr_stmt: &'b ExpressionStatement<'a>,
) -> Option<(&'b str, &'b Argument<'a>)> {
    let Expression::CallExpression(call) = &expr_stmt.expression else {
        return None;
    };
    if call.arguments.len() != 1 {
        return None;
    }
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return None;
    };
    if member.property.name.as_str() != "push" {
        return None;
    }
    let Expression::Identifier(obj) = &member.object else {
        return None;
    };
    Some((obj.name.as_str(), &call.arguments[0]))
}

/// Match a statement list against the two allowed "push-only" shapes
/// (see the check's doc comment). Returns the accumulator array's name.
fn match_push_only<'a>(stmts: &[&Statement<'a>]) -> Option<String> {
    match stmts {
        [Statement::ExpressionStatement(expr_stmt)] => {
            let (target, _arg) = extract_push(expr_stmt)?;
            Some(target.to_string())
        }
        [Statement::VariableDeclaration(decl), Statement::ExpressionStatement(expr_stmt)] => {
            if decl.declarations.len() != 1 {
                return None;
            }
            if !matches!(
                decl.kind,
                VariableDeclarationKind::Const | VariableDeclarationKind::Let
            ) {
                return None;
            }
            let d = &decl.declarations[0];
            let BindingPattern::BindingIdentifier(id) = &d.id else {
                return None;
            };
            d.init.as_ref()?; // must be a computed value, not a bare declaration
            let (target, arg) = extract_push(expr_stmt)?;
            match arg {
                Argument::Identifier(arg_id) if arg_id.name.as_str() == id.name.as_str() => {
                    Some(target.to_string())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

impl<'a> Collector<'a> {
    fn check_loop_body(&mut self, body: &Statement<'a>, loop_span: Span) {
        let stmts = body_statements(body);

        if let Some(target) = match_push_only(&stmts) {
            self.flag(loop_span, &target, "map");
            return;
        }

        if let [Statement::IfStatement(if_stmt)] = stmts.as_slice() {
            if if_stmt.alternate.is_some() {
                return; // an else branch isn't a plain filter
            }
            let inner = body_statements(&if_stmt.consequent);
            if let Some(target) = match_push_only(&inner) {
                self.flag(loop_span, &target, "filter");
            }
        }
    }

    fn flag(&mut self, loop_span: Span, target: &str, method: &str) {
        let span = span_from_bytes(&self.file.text, loop_span.start, loop_span.end);
        self.issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!(
                "this loop only builds `{target}` by pushing one computed value per \
                 iteration — could be written with `.{method}()`"
            ),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(META.base_priority),
            severity: Severity::Low,
            related: Vec::new(),
        });
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_for_statement(&mut self, node: &ForStatement<'a>) {
        self.check_loop_body(&node.body, node.span());
        oxc_ast_visit::walk::walk_for_statement(self, node);
    }

    fn visit_for_of_statement(&mut self, node: &ForOfStatement<'a>) {
        self.check_loop_body(&node.body, node.span());
        oxc_ast_visit::walk::walk_for_of_statement(self, node);
    }
}
