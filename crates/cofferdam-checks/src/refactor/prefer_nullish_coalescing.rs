use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{Expression, LogicalExpression, LogicalOperator};
use oxc_ast_visit::Visit;

// ─── Refactor.PreferNullishCoalescing ──────────────────────────────────────
//
// Modern TS idiom: `x ?? default` instead of `x || default` when the
// intent is "fall through on null/undefined only". The precise rule needs
// type info — `x || 0` is genuinely correct when `x` can be a meaningful
// `0`. We ship the narrow high-confidence shape now: `member-access ||
// literal-default`, and broaden in the type-aware tier.
//
// What we flag: `lhs || rhs` where lhs is a member access (`obj.prop` or
// `obj[key]`) AND rhs is a default-shaped expression — string/number/bool
// literal, `null`, the bare identifier `undefined`, an array literal, or
// an object literal.
//
// What we *don't* flag:
//   - `x || y`               (both sides bare identifiers — could be alt branch)
//   - `getValue() || 0`      (function call — return type ambiguous)
//   - `(a + b) || 0`         (arithmetic — explicit falsy-fallthrough intent)
//   - `obj.prop || other()`  (RHS is a call — not a "default" shape)
//   - `obj.prop || flag`     (RHS is a bare identifier — likely alt branch)

/// `Refactor.PreferNullishCoalescing` — flags `lhs || default`
/// patterns where `lhs ?? default` would be safer (avoids treating
/// falsy-but-valid values like `0` or `""` as missing). See
/// `CheckMeta` for the exact shape the check matches.
pub struct PreferNullishCoalescing;

const PREFER_NULLISH_META: CheckMeta = CheckMeta {
    id: "Refactor.PreferNullishCoalescing",
    category: Category::Refactor,
    base_priority: 3,
    default_severity: Severity::Low,
    explanation: "`x || default` falls through on every falsy value (`0`, `\"\"`, `false`). Use `??` to fall through only on `null`/`undefined`.",
    body: include_str!("../../docs/Refactor.PreferNullishCoalescing.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for PreferNullishCoalescing {
    fn meta(&self) -> &'static CheckMeta {
        &PREFER_NULLISH_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = NullishVisitor {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct NullishVisitor<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

/// Is this expression a "default-shaped" literal? Used as the RHS gate in
/// `PreferNullishCoalescing`. Conservative — we'd rather miss a true
/// positive than flag a falsy-fallthrough that the user wrote on purpose.
fn is_default_literal(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::ArrayExpression(_)
        | Expression::ObjectExpression(_) => true,
        // `undefined` is parsed as an IdentifierReference, not a keyword.
        Expression::Identifier(ident) => ident.name.as_str() == "undefined",
        _ => false,
    }
}

/// Is this expression a member access we'd want `??` to chain off? We
/// flag only on member-access LHS because bare identifiers (`flag ||
/// other`) are too often genuine alternative-branch logic to flag without
/// types. Function-call LHS (`get() || 0`) is also out of scope for the
/// same reason — return type ambiguity.
fn is_member_access(expr: &Expression<'_>) -> bool {
    matches!(
        expr,
        Expression::StaticMemberExpression(_) | Expression::ComputedMemberExpression(_)
    )
}

impl<'a> Visit<'a> for NullishVisitor<'a> {
    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
        if matches!(node.operator, LogicalOperator::Or)
            && is_member_access(&node.left)
            && is_default_literal(&node.right)
        {
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.issues.push(Issue {
                check_id: PREFER_NULLISH_META.id.to_string(),
                message: "prefer nullish coalescing `??` for default values (`||` falls through on `0`/`\"\"`/`false`)".to_string(),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(PREFER_NULLISH_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
        oxc_ast_visit::walk::walk_logical_expression(self, node);
    }
}
