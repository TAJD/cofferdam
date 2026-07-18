use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    Argument, ArrowFunctionExpression, AssignmentTarget, BindingPattern, CallExpression,
    Expression, FormalParameters, Function, SimpleAssignmentTarget, Statement, TSType, TSTypeName,
    TSTypeOperatorOperator, UpdateExpression,
};
use oxc_ast_visit::Visit;

/// In-place array-mutating method names — a call to one of these on the
/// parameter (`items.push(x)`) counts as a mutation even though it isn't
/// an assignment expression.
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

/// `Design.ReadonlyArrayParam` — flag a function parameter declared with a
/// mutable array (`T[]`, `Array<T>`) or object-literal (`{ x: number }`)
/// type where nothing in the function body mutates it, suggesting
/// `readonly T[]`/`ReadonlyArray<T>`/`Readonly<T>` instead.
///
/// AST-only (declared-type syntax, not inferred types): a parameter's type
/// annotation is already present in the source, so no ts-morph round-trip
/// is needed to read it back.
///
/// Conservative by design (per the ticket): a parameter that is ever
/// passed as an argument to another call is skipped entirely, since a
/// callee might mutate it and this check can't see across the call graph.
/// Only single-level assignment/index writes and array-mutating method
/// calls on the parameter's own root identifier are checked — an
/// assignment via a nested/destructured alias is not tracked, mirroring
/// `Refactor.MutatedParameter`'s scope.
pub struct ReadonlyArrayParam;

const META: CheckMeta = CheckMeta {
    id: "Design.ReadonlyArrayParam",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Low,
    explanation: "A function parameter typed as a mutable array or object, but never mutated \
        in the body, is a missed `readonly` guarantee — a type-checker-enforced promise to \
        callers that's cheap to add and cheap for them to trust.",
    body: include_str!("../../docs/Design.ReadonlyArrayParam.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for ReadonlyArrayParam {
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

#[derive(Clone, Copy)]
enum CandidateKind {
    Array,
    Object,
}

impl CandidateKind {
    fn suggestion(self) -> &'static str {
        match self {
            CandidateKind::Array => "readonly T[]` / `ReadonlyArray<T>",
            CandidateKind::Object => "Readonly<T>",
        }
    }
}

/// Whether `ty` is a candidate mutable array/object type — and not
/// already wrapped in a `readonly` modifier or `Readonly<T>`/
/// `ReadonlyArray<T>` reference.
fn candidate_kind(ty: &TSType<'_>) -> Option<CandidateKind> {
    match ty {
        TSType::TSArrayType(_) => Some(CandidateKind::Array),
        TSType::TSTypeLiteral(_) => Some(CandidateKind::Object),
        TSType::TSTypeReference(r) => match &r.type_name {
            TSTypeName::IdentifierReference(id) if id.name.as_str() == "Array" => {
                Some(CandidateKind::Array)
            }
            _ => None,
        },
        // `readonly T[]` / `readonly { ... }` — already immutable, not a
        // candidate regardless of the wrapped type.
        TSType::TSTypeOperatorType(op) if op.operator == TSTypeOperatorOperator::Readonly => None,
        _ => None,
    }
}

struct ParamCandidate {
    name: String,
    kind: CandidateKind,
    span_start: u32,
    span_end: u32,
}

fn param_candidates(params: &FormalParameters<'_>) -> Vec<ParamCandidate> {
    let mut out = Vec::new();
    for p in &params.items {
        let BindingPattern::BindingIdentifier(id) = &p.pattern else {
            continue;
        };
        let Some(annotation) = &p.type_annotation else {
            continue;
        };
        let Some(kind) = candidate_kind(&annotation.type_annotation) else {
            continue;
        };
        out.push(ParamCandidate {
            name: id.name.as_str().to_string(),
            kind,
            span_start: p.span.start,
            span_end: p.span.end,
        });
    }
    out
}

/// Scans a function body (not descending into nested function/arrow
/// scopes) for mutation of, or pass-through of, a single named parameter.
struct ParamUsageScanner<'a> {
    name: &'a str,
    mutated: bool,
    passed_as_arg: bool,
}

impl<'a> ParamUsageScanner<'a> {
    fn is_target(&self, expr: &Expression<'_>) -> bool {
        matches!(expr, Expression::Identifier(id) if id.name.as_str() == self.name)
    }
}

impl<'a> Visit<'a> for ParamUsageScanner<'a> {
    fn visit_assignment_expression(&mut self, node: &oxc_ast::ast::AssignmentExpression<'a>) {
        let root = match &node.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => Some(id.name.as_str()),
            AssignmentTarget::StaticMemberExpression(member) => match &member.object {
                Expression::Identifier(id) => Some(id.name.as_str()),
                _ => None,
            },
            AssignmentTarget::ComputedMemberExpression(member) => match &member.object {
                Expression::Identifier(id) => Some(id.name.as_str()),
                _ => None,
            },
            _ => None,
        };
        if root == Some(self.name) {
            self.mutated = true;
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument {
            if id.name.as_str() == self.name {
                self.mutated = true;
            }
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }

    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &node.callee {
            if let Expression::Identifier(obj) = &member.object {
                if obj.name.as_str() == self.name
                    && ARRAY_MUTATING_METHODS.contains(&member.property.name.as_str())
                {
                    self.mutated = true;
                }
            }
        }
        for arg in &node.arguments {
            let is_target = match arg {
                Argument::SpreadElement(spread) => self.is_target(&spread.argument),
                Argument::Identifier(id) => id.name.as_str() == self.name,
                _ => false,
            };
            if is_target {
                self.passed_as_arg = true;
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_function(&mut self, _node: &Function<'a>, _flags: oxc_syntax::scope::ScopeFlags) {
        // Nested function — separate scope, not this parameter's own body.
    }

    fn visit_arrow_function_expression(&mut self, _node: &ArrowFunctionExpression<'a>) {
        // Same reasoning as visit_function above.
    }
}

fn scan_param_usage(name: &str, stmts: &[Statement<'_>]) -> (bool, bool) {
    let mut scanner = ParamUsageScanner {
        name,
        mutated: false,
        passed_as_arg: false,
    };
    for stmt in stmts {
        scanner.visit_statement(stmt);
    }
    (scanner.mutated, scanner.passed_as_arg)
}

struct Collector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Collector<'a> {
    fn check_params(&mut self, params: &FormalParameters<'_>, stmts: &[Statement<'_>]) {
        for candidate in param_candidates(params) {
            let (mutated, passed_as_arg) = scan_param_usage(&candidate.name, stmts);
            if mutated || passed_as_arg {
                continue;
            }
            let span = span_from_bytes(&self.file.text, candidate.span_start, candidate.span_end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "parameter `{}` is never mutated — consider `{}` for a type-checker-enforced guarantee",
                    candidate.name,
                    candidate.kind.suggestion()
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
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        if let Some(body) = &node.body {
            self.check_params(&node.params, &body.statements);
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.check_params(&node.params, &node.body.statements);
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }
}
