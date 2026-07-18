use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, BindingPattern, UpdateExpression, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::Visit;
use std::collections::{HashMap, HashSet};

/// `Refactor.PreferConstOverLet` — flag a `let` binding that is never
/// reassigned anywhere in the file.
///
/// Scoped to simple-identifier bindings only (destructured bindings are
/// skipped — MVP, mirrors `Refactor.MutatedParameter`). Reassignment is
/// tracked by name across the whole file rather than by true lexical
/// scope: a single pass collects every `let`-declared name and every
/// name that's ever the target of an `AssignmentExpression` or
/// `UpdateExpression`, anywhere, including inside nested closures — this
/// deliberately follows the ticket's false-positive watch (a naive
/// same-scope walk would miss a closure reassigning a captured outer
/// `let`, wrongly flagging it as never-reassigned).
///
/// The name-based tracking has a known, deliberately-accepted trade-off
/// in the other direction: a shadowed `let` with the same name as a
/// reassigned variable in a different scope is indistinguishable from
/// it, and won't be flagged even if that specific binding is never
/// itself reassigned. This is a false-negative, not a false-positive —
/// the safer direction for a "should be const" suggestion.
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
            let_decls: HashMap::new(),
            reassigned: HashSet::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.into_issues()
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    /// name -> first-seen declarator span. A `HashMap` (not `Vec`) so a
    /// re-declaration of the same name only reports once.
    let_decls: HashMap<String, (u32, u32)>,
    reassigned: HashSet<String>,
}

impl<'a> Collector<'a> {
    fn into_issues(self) -> Vec<Issue> {
        let mut issues: Vec<Issue> = self
            .let_decls
            .into_iter()
            .filter(|(name, _)| !self.reassigned.contains(name))
            .map(|(name, (start, end))| {
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

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_variable_declaration(&mut self, node: &VariableDeclaration<'a>) {
        if node.kind == VariableDeclarationKind::Let {
            for decl in &node.declarations {
                if let BindingPattern::BindingIdentifier(id) = &decl.id {
                    self.let_decls
                        .entry(id.name.as_str().to_string())
                        .or_insert((decl.span.start, decl.span.end));
                }
            }
        }
        oxc_ast_visit::walk::walk_variable_declaration(self, node);
    }

    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &node.left {
            self.reassigned.insert(id.name.as_str().to_string());
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &node.argument
        {
            self.reassigned.insert(id.name.as_str().to_string());
        }
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }
}
