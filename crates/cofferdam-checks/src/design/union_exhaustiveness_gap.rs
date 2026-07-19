use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{Expression, SwitchStatement};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;
use std::collections::HashSet;

/// `Design.UnionExhaustivenessGap` — flag a `switch` over a discriminant
/// whose type is a literal union (a discriminated union's tag) that
/// doesn't cover every member and has no `default` case.
///
/// Type-aware: routes through the ts-morph type host via
/// [`cofferdam_core::TypeOracle::union_members_at`] (CD-118). Bails
/// entirely — no finding — when the discriminant's type isn't a
/// literal-only union, which naturally excludes non-discriminated unions
/// (`string`, `string | number`, a union of object types queried on the
/// object itself rather than its tag property).
///
/// A `default` case (of any content) is treated as an intentional
/// catch-all and suppresses the finding — this deliberately doesn't
/// require the `const _exhaustive: never = x` idiom TypeScript's own
/// compiler already flags in strict mode; requiring it here would
/// double-report what `tsc` already catches.
///
/// Scoped to `switch` statements only in this first version — exhaustive
/// `if`/`else if` chains over a discriminant are a known gap, deferred
/// until there's a cheap way to recognise the pattern without excessive
/// false positives on unrelated `if`-chains.
///
/// Case tests are matched against string, numeric, and boolean literals,
/// same as the type-host's `unionMembers` reports member values (each
/// stringified via `String(value)`). A case test that isn't a literal at
/// all (e.g. a computed expression) can't be attributed to a member and
/// is silently ignored rather than treated as covering anything.
pub struct UnionExhaustivenessGap;

const META: CheckMeta = CheckMeta {
    id: "Design.UnionExhaustivenessGap",
    category: Category::Design,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "A switch over a discriminated union's tag doesn't handle every variant and \
        has no default case — adding a new variant later can silently fall through unhandled.",
    body: include_str!("../../docs/Design.UnionExhaustivenessGap.md"),
    requires_types: true,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for UnionExhaustivenessGap {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        // Type-aware: the engine only routes us here when an oracle is
        // installed, but guard defensively rather than unwrap.
        let Some(types) = ctx.types else {
            return Vec::new();
        };
        let mut visitor = Collector {
            file,
            types,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    types: &'a dyn cofferdam_core::TypeOracle,
    issues: Vec<Issue>,
}

impl<'a> Collector<'a> {
    fn check_switch(&mut self, node: &SwitchStatement<'a>) {
        let disc_span = node.discriminant.span();
        let Some(union) =
            self.types
                .union_members_at(&self.file.path, disc_span.start, disc_span.end)
        else {
            return; // not a literal-only union discriminant
        };
        if union.members.is_empty() {
            return;
        }

        let mut handled: HashSet<String> = HashSet::new();
        for case in &node.cases {
            match &case.test {
                None => return, // explicit `default` — intentional catch-all
                Some(Expression::StringLiteral(lit)) => {
                    handled.insert(lit.value.as_str().to_string());
                }
                // `unionMembers` stringifies numeric/boolean literal members
                // the same way (`String(value)`), so match that here.
                Some(Expression::NumericLiteral(lit)) => {
                    handled.insert(lit.value.to_string());
                }
                Some(Expression::BooleanLiteral(lit)) => {
                    handled.insert(lit.value.to_string());
                }
                Some(_) => {} // non-literal case test — can't attribute to a member
            }
        }

        let missing: Vec<&str> = union
            .members
            .iter()
            .filter(|m| !handled.contains(*m))
            .map(String::as_str)
            .collect();
        if missing.is_empty() {
            return;
        }

        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
        self.issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!(
                "switch over a discriminated union doesn't handle every variant and has no \
                 default case — missing: {}",
                missing.join(", ")
            ),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(META.base_priority),
            severity: Severity::Medium,
            related: Vec::new(),
        });
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_switch_statement(&mut self, node: &SwitchStatement<'a>) {
        self.check_switch(node);
        oxc_ast_visit::walk::walk_switch_statement(self, node);
    }
}
