use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, AssignmentTarget, BindingPattern, Expression,
    FormalParameters, Function, SimpleAssignmentTarget, UpdateExpression,
};
use oxc_ast_visit::Visit;
use std::collections::HashSet;

/// `Refactor.MutatedParameter` — flag a function that reassigns or mutates
/// one of its own parameter bindings.
///
/// Scoped to simple-identifier parameters only (destructured/rest params
/// are skipped — MVP). Watches for direct reassignment (`x = ...`),
/// single-level member mutation (`x.foo = ...`), single-level index
/// mutation (`x[0] = ...`), and increment/decrement (`x++`). Nested
/// property writes (`x.a.b = ...`) and `delete x.foo` are not detected.
///
/// The watch set is a stack, one per enclosing function-like scope: a
/// nested function/arrow inherits every enclosing function's watched
/// param names (so a closure mutating an *outer* param still flags).
/// Tracking is name-based, not binding-based, so a nested local that
/// happens to share a name with an enclosing param is indistinguishable
/// from that param and will also flag if reassigned — a known,
/// deliberately-accepted false-positive edge case for this first version.
pub struct MutatedParameter;

const META: CheckMeta = CheckMeta {
    id: "Refactor.MutatedParameter",
    category: Category::Refactor,
    base_priority: 10,
    default_severity: Severity::Medium,
    explanation: "Reassigning or mutating a function parameter breaks pure \
        input\u{2192}output semantics, making the function harder to test and reason \
        about in isolation.",
    body: include_str!("../../docs/Refactor.MutatedParameter.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for MutatedParameter {
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
            scopes: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

fn simple_param_names(params: &FormalParameters) -> HashSet<String> {
    let mut names = HashSet::new();
    for p in &params.items {
        if let BindingPattern::BindingIdentifier(id) = &p.pattern {
            names.insert(id.name.as_str().to_string());
        }
    }
    names
}

struct Collector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
    /// One watch set per enclosing function-like scope, innermost last.
    scopes: Vec<HashSet<String>>,
}

impl<'a> Collector<'a> {
    fn push_scope(&mut self, own_params: HashSet<String>) {
        let mut scope = self.scopes.last().cloned().unwrap_or_default();
        scope.extend(own_params);
        self.scopes.push(scope);
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn flag(&mut self, name: &str, verb: &str, span_start: u32, span_end: u32) {
        let span = span_from_bytes(&self.file.text, span_start, span_end);
        self.issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!("parameter `{name}` is {verb} inside the function body"),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(META.base_priority),
            severity: Severity::Medium,
            related: Vec::new(),
        });
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        self.push_scope(simple_param_names(&node.params));
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.pop_scope();
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.push_scope(simple_param_names(&node.params));
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.pop_scope();
    }

    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        if let Some(watched) = self.scopes.last().cloned() {
            match &node.left {
                AssignmentTarget::AssignmentTargetIdentifier(id) => {
                    let name = id.name.as_str();
                    if watched.contains(name) {
                        self.flag(name, "reassigned", node.span.start, node.span.end);
                    }
                }
                AssignmentTarget::StaticMemberExpression(member) => {
                    if let Expression::Identifier(obj) = &member.object {
                        let name = obj.name.as_str();
                        if watched.contains(name) {
                            self.flag(
                                name,
                                "mutated (property write)",
                                node.span.start,
                                node.span.end,
                            );
                        }
                    }
                }
                AssignmentTarget::ComputedMemberExpression(member) => {
                    if let Expression::Identifier(obj) = &member.object {
                        let name = obj.name.as_str();
                        if watched.contains(name) {
                            self.flag(
                                name,
                                "mutated (index write)",
                                node.span.start,
                                node.span.end,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let Some(watched) = self.scopes.last().cloned() {
            if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument {
                let name = id.name.as_str();
                if watched.contains(name) {
                    self.flag(
                        name,
                        "mutated (increment/decrement)",
                        node.span.start,
                        node.span.end,
                    );
                }
            }
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }
}
