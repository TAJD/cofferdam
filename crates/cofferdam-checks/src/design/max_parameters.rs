use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, OptionDefault, OptionKind,
    OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{ArrowFunctionExpression, Function};
use oxc_ast_visit::Visit;

/// `Design.MaxParameters` — flag function signatures over `limit` params.
///
/// Counts logical parameter slots (an `{a, b}` destructure is 1, a `...rest`
/// is 1, a default-valued param is 1). Cheap counter check that exercises
/// the same SDK seam as TripleEquals from a different angle (visiting
/// function-like nodes rather than expressions).
pub struct MaxParameters {
    limit: u32,
    meta: &'static CheckMeta,
}

const MP_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(5),
    doc: "maximum number of parameters per function signature",
}];

const META: CheckMeta = CheckMeta {
    id: "Design.MaxParameters",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "Functions with too many parameters are hard to call correctly. Pass an options object instead.",
    body: include_str!("../../docs/Design.MaxParameters.md"),
    requires_types: false,
    consistency: false,
    options: MP_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl MaxParameters {
    /// Construct with a parameter-count ceiling. `all_builtins`
    /// installs the default of 5; user config overrides via
    /// `[checks."Design.MaxParameters"].limit`.
    pub fn new(limit: u32) -> Self {
        Self { limit, meta: &META }
    }
}

impl Check for MaxParameters {
    fn meta(&self) -> &'static CheckMeta {
        self.meta
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let limit = ctx
            .options
            .get_int("limit")
            .map(|v| v as u32)
            .unwrap_or(self.limit);
        let mut visitor = Collector {
            file,
            limit,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
}

impl<'a> Collector<'a> {
    fn check_params(&mut self, count: usize, name: &str, span_start: u32, span_end: u32) {
        if count as u32 > self.limit {
            let span = span_from_bytes(&self.file.text, span_start, span_end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "{} has {} parameters, exceeds limit of {}",
                    name, count, self.limit
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        self.check_params(
            node.params.items.len(),
            &name,
            node.span.start,
            node.span.end,
        );
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.check_params(
            node.params.items.len(),
            "arrow function",
            node.span.start,
            node.span.end,
        );
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }
}
