use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    Argument, AssignmentTarget, BindingPattern, CallExpression, Expression, FormalParameters,
    Function, SimpleAssignmentTarget, Statement, UpdateExpression, VariableDeclaration,
};
use oxc_ast_visit::Visit;
use std::collections::HashSet;

/// Member names on `.map`/`.filter` calls whose callback argument is
/// inspected. `.forEach` is deliberately excluded — a side effect there
/// is the whole point, not a smell.
const MAP_LIKE_METHODS: &[&str] = &["map", "filter"];

/// `.<name>()` calls treated as inherently side-effecting.
const SIDE_EFFECTING_OBJECTS: &[&str] = &["console"];

/// In-place array-mutating method names. A call to one of these on a
/// non-local identifier (`seen.push(item)`) is the idiomatic way
/// "mutates an outer-scope array" actually shows up in real code — an
/// assignment-target check alone wouldn't catch it.
const ARRAY_MUTATING_METHODS: &[&str] = &[
    "push",
    "pop",
    "shift",
    "unshift",
    "splice",
    "sort",
    "reverse",
    "fill",
    "copyWithin",
];

/// `Refactor.SideEffectInMapCallback` — flag a callback passed to
/// `.map`/`.filter` that mutates something outside itself (an
/// outer-scope variable) or calls a known side-effecting function
/// (`console.*`), instead of purely computing and returning a value —
/// "a loop wearing a map costume."
///
/// AST-only, name-based (mirrors `Refactor.UnusedVariable`'s
/// corpus/mutation-tracking approach, scoped to a single callback
/// instead of a whole file):
/// - Only the callback's own body is inspected — a nested function or
///   arrow declared inside the callback is a separate scope and is not
///   descended into, so its mutations don't count against the outer
///   callback.
/// - "Mutates something outside itself" is name-based: an assignment
///   target, an update-expression target, or the receiver of an
///   in-place array-mutating method call (`.push`, `.splice`, ...) is
///   treated as external unless its name is one of the callback's own
///   parameters or a `var`/`let`/`const` declared directly in the
///   callback's own body. A local that happens to share a name with
///   something outside is indistinguishable from the real thing — an
///   accepted false-negative/positive edge case for a fuzzy heuristic.
/// - `console.*` is the only side-effecting-call denylist entry beyond
///   array mutation (per the ticket's v1 scope) — other known
///   side-effecting calls (I/O, DOM mutation, etc.) are not detected.
/// - A `.map`/`.filter` callback whose return value is simply discarded
///   (a `.forEach`-shaped misuse with no other side effect) is NOT
///   flagged by this check — that's a distinct "use forEach instead"
///   concern the ticket explicitly says not to conflate with this one.
pub struct SideEffectInMapCallback;

const META: CheckMeta = CheckMeta {
    id: "Refactor.SideEffectInMapCallback",
    category: Category::Refactor,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A .map/.filter callback that mutates outer-scope state or calls a known \
        side-effecting function isn't purely computing a value — it's a loop wearing a map \
        costume.",
    body: include_str!("../../docs/Refactor.SideEffectInMapCallback.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for SideEffectInMapCallback {
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

/// A callback's own parameter + directly-declared local names (not
/// crossing into any nested function's own scope).
fn local_names_from_params(params: &FormalParameters<'_>) -> HashSet<String> {
    let mut names = HashSet::new();
    for p in &params.items {
        if let BindingPattern::BindingIdentifier(id) = &p.pattern {
            names.insert(id.name.as_str().to_string());
        }
    }
    names
}

struct LocalDeclCollector {
    names: HashSet<String>,
}

impl<'a> Visit<'a> for LocalDeclCollector {
    fn visit_variable_declaration(&mut self, node: &VariableDeclaration<'a>) {
        for d in &node.declarations {
            if let BindingPattern::BindingIdentifier(id) = &d.id {
                self.names.insert(id.name.as_str().to_string());
            }
        }
        oxc_ast_visit::walk::walk_variable_declaration(self, node);
    }

    fn visit_function(&mut self, _node: &Function<'a>, _flags: oxc_syntax::scope::ScopeFlags) {
        // Nested function — separate scope, its own locals don't belong
        // to the outer callback.
    }

    fn visit_arrow_function_expression(
        &mut self,
        _node: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        // Same reasoning as visit_function above.
    }
}

/// True if `stmts` (a callback's own body, not descending into nested
/// function/arrow scopes) contains an assignment/update targeting a
/// name outside `locals`, or a call to a denylisted side-effecting
/// object member (`console.*`).
fn has_side_effect(stmts: &[Statement<'_>], locals: &HashSet<String>) -> bool {
    let mut scanner = SideEffectScanner {
        locals,
        found: false,
    };
    for stmt in stmts {
        scanner.visit_statement(stmt);
        if scanner.found {
            return true;
        }
    }
    false
}

struct SideEffectScanner<'a> {
    locals: &'a HashSet<String>,
    found: bool,
}

impl<'a> SideEffectScanner<'a> {
    fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }
}

impl<'a> Visit<'a> for SideEffectScanner<'a> {
    fn visit_assignment_expression(&mut self, node: &oxc_ast::ast::AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &node.left {
            if !self.is_local(id.name.as_str()) {
                self.found = true;
            }
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument {
            if !self.is_local(id.name.as_str()) {
                self.found = true;
            }
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }

    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &node.callee {
            if let Expression::Identifier(obj) = &member.object {
                let name = obj.name.as_str();
                let property = member.property.name.as_str();
                let is_denylisted_call = SIDE_EFFECTING_OBJECTS.contains(&name);
                let is_external_mutation =
                    !self.is_local(name) && ARRAY_MUTATING_METHODS.contains(&property);
                if is_denylisted_call || is_external_mutation {
                    self.found = true;
                }
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_function(&mut self, _node: &Function<'a>, _flags: oxc_syntax::scope::ScopeFlags) {
        // Nested function — separate scope, not this callback's own
        // mutations/calls.
    }

    fn visit_arrow_function_expression(
        &mut self,
        _node: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        // Same reasoning as visit_function above.
    }
}

fn check_callback_stmts(params: &FormalParameters<'_>, stmts: &[Statement<'_>]) -> bool {
    let mut locals = local_names_from_params(params);
    let mut decl_collector = LocalDeclCollector {
        names: HashSet::new(),
    };
    for stmt in stmts {
        decl_collector.visit_statement(stmt);
    }
    locals.extend(decl_collector.names);
    has_side_effect(stmts, &locals)
}

impl<'a> Collector<'a> {
    fn check_map_like_call(&mut self, node: &CallExpression<'a>) {
        let Expression::StaticMemberExpression(member) = &node.callee else {
            return;
        };
        if !MAP_LIKE_METHODS.contains(&member.property.name.as_str()) {
            return;
        }
        let Some(first_arg) = node.arguments.first() else {
            return;
        };
        let flagged = match first_arg {
            Argument::FunctionExpression(func) => {
                let Some(body) = &func.body else {
                    return;
                };
                check_callback_stmts(&func.params, &body.statements)
            }
            Argument::ArrowFunctionExpression(arrow) => {
                check_callback_stmts(&arrow.params, &arrow.body.statements)
            }
            _ => return,
        };
        if flagged {
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "`.{}` callback has a side effect (mutates outer-scope state or calls \
                     `console.*`) instead of purely computing a value",
                    member.property.name.as_str()
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(META.base_priority),
                severity: Severity::Low,
                related: Vec::new(),
            });
        }
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        self.check_map_like_call(node);
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }
}
