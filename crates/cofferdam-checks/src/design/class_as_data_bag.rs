use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{Class, ClassElement, Expression, MethodDefinitionKind, Statement};
use oxc_ast_visit::Visit;

/// `Design.ClassAsDataBag` — flag a class with no behavior beyond storing
/// fields: no methods other than a constructor that only assigns fields
/// (`this.x = x;`), no inheritance, no `implements` clause, no
/// decorators, and no constructor parameter properties. Candidate for a
/// plain type/interface instead of a class.
///
/// AST-only, despite living alongside type-aware Design checks — every
/// escape hatch below is syntactic (parameter property modifiers,
/// `implements`/`extends` presence, decorator presence), so no type
/// host query is needed.
///
/// Escape hatches (do NOT flag when any hold):
/// - Constructor uses parameter properties (`constructor(private x: T)`)
///   — the standard DI pattern without decorators.
/// - Class has an `implements` clause — a structural contract, not just
///   a data bag.
/// - Class has a superclass — subclassing (including built-ins like
///   `Error`) is a legitimate non-FP use. This is really just a
///   restatement of "no inheritance" from the main criterion below, not
///   an independent check.
/// - Class has a decorator.
/// - The constructor body contains anything besides `this.<field> =
///   <expr>;` statements (a conditional, a method call, validation
///   logic) — that's behavior, not a data bag.
///
/// Scope note: a class with no constructor at all is only flagged if it
/// has at least one plain field (`PropertyDefinition`) — a genuinely
/// empty class (`class Marker {}`) is a nominal-type marker pattern, a
/// different concern this check doesn't address.
pub struct ClassAsDataBag;

const META: CheckMeta = CheckMeta {
    id: "Design.ClassAsDataBag",
    category: Category::Design,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A class with no behavior beyond storing fields — no methods, no inheritance, \
        no implements clause — is a candidate for a plain type/interface instead.",
    body: include_str!("../../docs/Design.ClassAsDataBag.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for ClassAsDataBag {
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

/// True if `stmt` is exactly `this.<field> = <expr>;` — plain `=` only.
/// A compound operator (`+=`, `??=`, ...) is behavior (it reads before
/// writing), not initialization, so it must NOT count as a field
/// assignment even though its target still looks like `this.<field>`.
fn is_this_field_assignment(stmt: &Statement<'_>) -> bool {
    let Statement::ExpressionStatement(expr_stmt) = stmt else {
        return false;
    };
    let Expression::AssignmentExpression(assign) = &expr_stmt.expression else {
        return false;
    };
    if assign.operator != oxc_ast::ast::AssignmentOperator::Assign {
        return false;
    }
    let oxc_ast::ast::AssignmentTarget::StaticMemberExpression(member) = &assign.left else {
        return false;
    };
    matches!(member.object, Expression::ThisExpression(_))
}

impl<'a> Collector<'a> {
    fn check_class(&mut self, node: &Class<'a>) {
        if node.super_class.is_some() || !node.implements.is_empty() || !node.decorators.is_empty()
        {
            return;
        }

        let mut constructor: Option<&oxc_ast::ast::MethodDefinition<'a>> = None;
        let mut has_property_field = false;

        for element in &node.body.body {
            match element {
                ClassElement::MethodDefinition(m) => {
                    if m.kind == MethodDefinitionKind::Constructor {
                        if constructor.is_some() {
                            return; // malformed (duplicate ctor) — don't guess
                        }
                        constructor = Some(m);
                    } else {
                        return; // any non-constructor method disqualifies
                    }
                }
                ClassElement::AccessorProperty(_) => return, // accessor = behavior
                ClassElement::StaticBlock(_) => return,      // static init block = behavior
                ClassElement::PropertyDefinition(_) => has_property_field = true,
                ClassElement::TSIndexSignature(_) => {}
            }
        }

        match constructor {
            Some(ctor) => {
                let has_param_property = ctor
                    .value
                    .params
                    .items
                    .iter()
                    .any(|p| p.accessibility.is_some() || p.readonly);
                if has_param_property {
                    return;
                }
                let Some(body) = &ctor.value.body else {
                    return; // no body (overload signature) — nothing to judge
                };
                if !body.statements.iter().all(is_this_field_assignment) {
                    return; // constructor does more than assign fields
                }
            }
            None => {
                if !has_property_field {
                    return; // no constructor and no fields — nothing to flag
                }
            }
        }

        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "(anonymous)".to_string());
        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
        self.issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!(
                "class `{name}` has no behavior beyond storing fields — consider a plain \
                 type/interface instead"
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
    fn visit_class(&mut self, node: &Class<'a>) {
        self.check_class(node);
        oxc_ast_visit::walk::walk_class(self, node);
    }
}
