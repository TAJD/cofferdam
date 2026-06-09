use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{Expression, LogicalExpression, LogicalOperator};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

// ─── Refactor.PreferOptionalChain ──────────────────────────────────────────
//
// Modern TS idiom: `a && a.b && a.b.c` → `a?.b?.c`. The fully precise rule
// needs type info (knowing whether `a` can be `0`/`""` and whether falling
// through on those values matters); we ship the high-confidence syntactic
// shape now and broaden in the type-aware tier (cd-moj's phase-5 follow-up).
//
// What we flag: `lhs && rhs` where `rhs` is a member access (or call on a
// member access) whose *object* span renders to the same source text as the
// `lhs` span. That catches:
//   - `a && a.b`            (left = identifier, right = static member)
//   - `a.b && a.b.c`        (extends the chain by one step)
//   - `a && a[0]`           (computed member)
//   - `a && a.b()`          (call on member access)
//   - `a && a.b && a.b.c`   (parses left-associative; both sub-`&&`s flag)
//
// What we *don't* flag:
//   - `a && b.c`            (different prefixes — clearly not a chain)
//   - `a && (a as any).b`   (parens / casts mean LHS text doesn't match)
//   - `a() && a().b`        (LHS is a call — repeating side-effects matters,
//                            so `?.` isn't a safe rewrite without types)
//
// Source-text comparison (rather than AST equivalence) is deliberate: it
// keeps false positives near zero and avoids reimplementing identifier
// resolution. A check that requires the *same exact bytes* on both sides
// of `&&` won't be confused by whitespace tweaks or comments.

/// `Refactor.PreferOptionalChain` — flags `a && a.b` patterns that
/// would be clearer as `a?.b`. See `CheckMeta` for the full matching
/// rules and known false-positive shapes the check declines to flag.
pub struct PreferOptionalChain;

const PREFER_OPTIONAL_CHAIN_META: CheckMeta = CheckMeta {
    id: "Refactor.PreferOptionalChain",
    category: Category::Refactor,
    base_priority: 5,
    default_severity: Severity::Low,
    explanation: "`a && a.b && a.b.c` is more concisely written as `a?.b?.c`. The optional-chain operator (`?.`) short-circuits on null/undefined.",
    body: include_str!("../../docs/Refactor.PreferOptionalChain.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for PreferOptionalChain {
    fn meta(&self) -> &'static CheckMeta {
        &PREFER_OPTIONAL_CHAIN_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = OptionalChainVisitor {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct OptionalChainVisitor<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> OptionalChainVisitor<'a> {
    fn slice(&self, start: u32, end: u32) -> Option<&str> {
        self.file.text.get(start as usize..end as usize)
    }
}

/// True when the LHS of an `&&` chain can be safely repeated (i.e.
/// rewriting `lhs && lhs.foo` to `lhs?.foo` doesn't change semantics).
/// Identifiers and pure member chains (no calls anywhere) qualify;
/// anything containing a `CallExpression` or `NewExpression` does not —
/// repeating it would either double-invoke a side-effecting function or
/// halve it after rewrite.
fn is_safe_chain_lhs(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Identifier(_) | Expression::ThisExpression(_) => true,
        Expression::StaticMemberExpression(m) => is_safe_chain_lhs(&m.object),
        Expression::ComputedMemberExpression(m) => {
            is_safe_chain_lhs(&m.object) && is_safe_chain_lhs(&m.expression)
        }
        // Inner `&&` chains (left-associative) — recurse on both sides.
        Expression::LogicalExpression(inner) if matches!(inner.operator, LogicalOperator::And) => {
            is_safe_chain_lhs(&inner.left) && is_safe_chain_lhs(&inner.right)
        }
        _ => false,
    }
}

impl<'a> Visit<'a> for OptionalChainVisitor<'a> {
    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
        if matches!(node.operator, LogicalOperator::And) && is_safe_chain_lhs(&node.left) {
            let lhs_span = node.left.span();
            // Find the "object" of the RHS — the part `?.` would chain off.
            let rhs_object_span = match &node.right {
                Expression::StaticMemberExpression(m) => Some(m.object.span()),
                Expression::ComputedMemberExpression(m) => Some(m.object.span()),
                Expression::CallExpression(c) => match &c.callee {
                    Expression::StaticMemberExpression(m) => Some(m.object.span()),
                    Expression::ComputedMemberExpression(m) => Some(m.object.span()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(rhs_obj_span) = rhs_object_span {
                let lhs_text = self.slice(lhs_span.start, lhs_span.end);
                let rhs_text = self.slice(rhs_obj_span.start, rhs_obj_span.end);
                if let (Some(l), Some(r)) = (lhs_text, rhs_text) {
                    if l == r {
                        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                        self.issues.push(Issue {
                            check_id: PREFER_OPTIONAL_CHAIN_META.id.to_string(),
                            message: format!(
                                "prefer optional chain `?.` over repeated `&&` on `{l}`"
                            ),
                            file: self.file.path.clone(),
                            location: Location::from_span(&self.file.path, span),
                            priority: Priority(PREFER_OPTIONAL_CHAIN_META.base_priority),
                            severity: Severity::Medium,
                            related: Vec::new(),
                        });
                    }
                }
            }
        }
        oxc_ast_visit::walk::walk_logical_expression(self, node);
    }
}
