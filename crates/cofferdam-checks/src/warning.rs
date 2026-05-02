//! Warning checks — likely bugs. Default severity is `Error` for this
//! category once the per-category severity defaults wire up (phase 3).

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{BinaryExpression, BinaryOperator};
use oxc_ast_visit::Visit;

/// `Warning.TripleEquals` — flags `==` and `!=` (vs `===` / `!==`).
///
/// Implementation note: pattern B from the SDK design (see
/// rovikore_host_credo_checks memory). Walks every BinaryExpression
/// and checks the operator. The visitor is one-shot per file; phase-1
/// engine creates a fresh allocator + parsed view per file, runs all
/// checks, drops everything.
pub struct TripleEquals;

const META: CheckMeta = CheckMeta {
    id: "Warning.TripleEquals",
    category: Category::Warning,
    base_priority: 15,
    explanation: "`==` and `!=` perform type coercion and are almost always a bug. Use `===` and `!==` instead.",
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for TripleEquals {
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

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_binary_expression(&mut self, node: &BinaryExpression<'a>) {
        if matches!(
            node.operator,
            BinaryOperator::Equality | BinaryOperator::Inequality
        ) {
            let preferred = match node.operator {
                BinaryOperator::Equality => "===",
                BinaryOperator::Inequality => "!==",
                _ => unreachable!(),
            };
            let actual = match node.operator {
                BinaryOperator::Equality => "==",
                BinaryOperator::Inequality => "!=",
                _ => unreachable!(),
            };
            // Point at the whole expression. A more precise span on the
            // operator token itself is a cd-81a.6 (autofix) follow-up.
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!("use `{}` instead of `{}`", preferred, actual),
                file: self.file.path.clone(),
                span,
                priority: Priority(META.base_priority),
                severity: Severity::Warning,
            });
        }
        // Walk into children — `==` can appear inside other binary ops
        // (e.g. `a && b == c`).
        oxc_ast_visit::walk::walk_binary_expression(self, node);
    }
}
