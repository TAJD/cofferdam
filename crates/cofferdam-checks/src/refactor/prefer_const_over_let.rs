use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetProperty,
    BindingPattern, ForStatementLeft, UpdateExpression, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::Visit;
use std::collections::HashSet;

/// `Refactor.PreferConstOverLet` — flag a `let` binding that is never
/// reassigned anywhere in the file.
///
/// Declaration sites are scoped to simple-identifier bindings only
/// (destructured `let` declarations like `let { a, b } = x;` are skipped
/// — MVP, mirrors `Refactor.MutatedParameter`). Reassignment, however,
/// also recognizes a `let`-declared name appearing inside a
/// destructuring-*assignment* target (`({ a, b } = x)` / `([a, b] = x)`),
/// not just a bare `name = x`. Reassignment is tracked by name across the
/// whole file rather than by true lexical scope: a single pass collects
/// every `let`-declared name and every name that's ever the target of an
/// `AssignmentExpression` or `UpdateExpression`, anywhere, including
/// inside nested closures — this deliberately follows the ticket's
/// false-positive watch (a naive same-scope walk would miss a closure
/// reassigning a captured outer `let`, wrongly flagging it as
/// never-reassigned).
///
/// The name-based tracking has a known, deliberately-accepted trade-off
/// in the other direction: a shadowed `let` with the same name as a
/// reassigned variable in a different scope is indistinguishable from
/// it, and won't be flagged even if that specific binding is never
/// itself reassigned. This is a false-negative, not a false-positive —
/// the safer direction for a "should be const" suggestion.
///
/// Every `let` declaration site is recorded independently (not
/// deduplicated by name), so two unrelated, never-reassigned `let`s
/// that happen to share a name (e.g. the same local name in two
/// different functions) are both flagged — only an actual reassignment
/// of that name suppresses the finding, and it suppresses it for every
/// site sharing the name, per the trade-off above.
pub struct PreferConstOverLet;

const META: CheckMeta = CheckMeta {
    id: "Refactor.PreferConstOverLet",
    category: Category::Refactor,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A `let` binding that's never reassigned should be `const` — it signals \
        the value doesn't change and rules out reassignment bugs at compile time.",
    body: include_str!("../../docs/Refactor.PreferConstOverLet.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for PreferConstOverLet {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = Collector {
            file,
            let_decls: Vec::new(),
            reassigned: HashSet::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.into_issues()
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    /// Every `let` declarator site: (name, span start, span end). Not
    /// deduplicated by name — two unrelated `let`s sharing a name (e.g.
    /// in different functions) are independent findings.
    let_decls: Vec<(String, u32, u32)>,
    reassigned: HashSet<String>,
}

impl<'a> Collector<'a> {
    fn into_issues(self) -> Vec<Issue> {
        let mut issues: Vec<Issue> = self
            .let_decls
            .into_iter()
            .filter(|(name, _, _)| !self.reassigned.contains(name))
            .map(|(name, start, end)| {
                let span = span_from_bytes(&self.file.text, start, end);
                Issue {
                    check_id: META.id.to_string(),
                    message: format!(
                        "`{name}` is declared with `let` but never reassigned — use `const`"
                    ),
                    file: self.file.path.clone(),
                    location: Location::from_span(&self.file.path, span),
                    priority: Priority(META.base_priority),
                    severity: Severity::Low,
                    related: Vec::new(),
                }
            })
            .collect();
        issues.sort_by_key(|i| i.location.line());
        issues
    }
}

/// Collect every identifier name bound by an assignment target, including
/// names nested inside object/array destructuring patterns (`({ a, b } =
/// x)`, `([a, b] = x)`), rest elements, and defaulted elements
/// (`([a = 1] = x)`) — not just a bare `name = x` identifier target.
fn collect_assignment_target_names(target: &AssignmentTarget<'_>, names: &mut Vec<String>) {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(id) => {
            names.push(id.name.as_str().to_string());
        }
        AssignmentTarget::ArrayAssignmentTarget(arr) => {
            for element in arr.elements.iter().flatten() {
                collect_maybe_default_names(element, names);
            }
            if let Some(rest) = &arr.rest {
                collect_assignment_target_names(&rest.target, names);
            }
        }
        AssignmentTarget::ObjectAssignmentTarget(obj) => {
            for prop in &obj.properties {
                collect_property_names(prop, names);
            }
            if let Some(rest) = &obj.rest {
                collect_assignment_target_names(&rest.target, names);
            }
        }
        // Member expressions (`obj.x = v`) aren't identifier bindings.
        // TS-wrapped targets (`(x as T) = v`, `(x satisfies T) = v`,
        // `x! = v`) DO wrap a nested `let`-declared identifier, but
        // there's no accessor from these variants back to their inner
        // expression's binding — a known, narrow false-positive gap
        // (rarer than the destructuring shapes this function targets).
        _ => {}
    }
}

fn collect_maybe_default_names(target: &AssignmentTargetMaybeDefault<'_>, names: &mut Vec<String>) {
    match target {
        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
            collect_assignment_target_names(&d.binding, names);
        }
        // `AssignmentTargetMaybeDefault` otherwise shares `AssignmentTarget`'s
        // variants (inherited via oxc's `inherit_variants!`) — reuse the
        // generated `as_assignment_target` conversion rather than
        // duplicating the array/object/identifier match arms.
        other => {
            if let Some(target) = other.as_assignment_target() {
                collect_assignment_target_names(target, names);
            }
        }
    }
}

fn collect_property_names(prop: &AssignmentTargetProperty<'_>, names: &mut Vec<String>) {
    match prop {
        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
            names.push(p.binding.name.as_str().to_string());
        }
        AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
            collect_maybe_default_names(&p.binding, names);
        }
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_variable_declaration(&mut self, node: &VariableDeclaration<'a>) {
        if node.kind == VariableDeclarationKind::Let {
            for decl in &node.declarations {
                if let BindingPattern::BindingIdentifier(id) = &decl.id {
                    self.let_decls.push((
                        id.name.as_str().to_string(),
                        decl.span.start,
                        decl.span.end,
                    ));
                }
            }
        }
        oxc_ast_visit::walk::walk_variable_declaration(self, node);
    }

    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        let mut names = Vec::new();
        collect_assignment_target_names(&node.left, &mut names);
        self.reassigned.extend(names);
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    /// A `for (x of xs)` / `for ([a, b] of xs)` / `for (x in obj)` loop head
    /// reassigns its target on every iteration but isn't an
    /// `AssignmentExpression` — it's a distinct grammar production — so it
    /// needs its own reassignment-collection entry point.
    fn visit_for_statement_left(&mut self, node: &ForStatementLeft<'a>) {
        if let Some(target) = node.as_assignment_target() {
            let mut names = Vec::new();
            collect_assignment_target_names(target, &mut names);
            self.reassigned.extend(names);
        }
        oxc_ast_visit::walk::walk_for_statement_left(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument
        {
            self.reassigned.insert(id.name.as_str().to_string());
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }
}
