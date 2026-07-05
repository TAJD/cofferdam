use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, OptionDefault, OptionKind,
    OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, ConditionalExpression, DoWhileStatement, ForInStatement,
    ForOfStatement, ForStatement, Function, IfStatement, LogicalExpression, Program,
    SwitchStatement, TryStatement, WhileStatement,
};
use oxc_ast_visit::Visit;

// ─── Refactor.CyclomaticComplexity ─────────────────────────────────────────

/// `Refactor.CyclomaticComplexity` — McCabe count per function.
///
/// Starts at 1 and adds 1 for every branching node: `if`, each non-default
/// `case`, `for`/`for..in`/`for..of`/`while`/`do..while`, ternary, `catch`,
/// and each `&&` / `||` / `??` in conditions. `else` alone does not add a
/// path. Emits when a function's count exceeds `limit`.
pub struct CyclomaticComplexity {
    limit: u32,
}

impl CyclomaticComplexity {
    /// Construct with a per-function complexity ceiling. `all_builtins`
    /// installs the default of 10; user config overrides via
    /// `[checks."Refactor.CyclomaticComplexity"].limit`.
    pub fn new(limit: u32) -> Self {
        Self { limit }
    }
}

const CYC_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(10),
    doc: "maximum cyclomatic complexity per function",
}];

const CYC_META: CheckMeta = CheckMeta {
    id: "Refactor.CyclomaticComplexity",
    category: Category::Refactor,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "McCabe cyclomatic complexity counts independent paths through a function. High values indicate branching that's hard to test and reason about.",
    body: include_str!("../../docs/Refactor.CyclomaticComplexity.md"),
    requires_types: false,
    consistency: false,
    options: CYC_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl Check for CyclomaticComplexity {
    fn meta(&self) -> &'static CheckMeta {
        &CYC_META
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
        let mut visitor = CycVisitor {
            file,
            limit,
            issues: Vec::new(),
            stack: Vec::new(),
            max: 0,
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

/// Highest per-function cyclomatic complexity found anywhere in the file,
/// independent of `limit` — used by `cofferdam advise --analyze` (CD-65
/// A4) to report current/remaining budget without needing a threshold.
/// Returns 0 for a file with no functions.
pub fn max_in_file(file: &SourceFile, program: &Program<'_>) -> u32 {
    let mut visitor = CycVisitor {
        file,
        limit: u32::MAX,
        issues: Vec::new(),
        stack: Vec::new(),
        max: 0,
    };
    visitor.visit_program(program);
    visitor.max
}

struct CycVisitor<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
    /// Per-function tally. Push 1 (McCabe's base) on entry, pop on exit.
    /// Nested functions get their own entry — outer function's tally is
    /// undisturbed by inner branching.
    stack: Vec<u32>,
    /// Highest count seen across any function so far, tracked regardless
    /// of `limit` so [`max_in_file`] can reuse this same visitor.
    max: u32,
}

impl<'a> CycVisitor<'a> {
    fn enter(&mut self) {
        self.stack.push(1);
    }

    fn exit(&mut self, name: String, span_start: u32, span_end: u32) {
        let count = self.stack.pop().unwrap_or(1);
        self.max = self.max.max(count);
        if count > self.limit {
            let span = span_from_bytes(&self.file.text, span_start, span_end);
            self.issues.push(Issue {
                check_id: CYC_META.id.to_string(),
                message: format!(
                    "{} has cyclomatic complexity {}, exceeds limit of {}",
                    name, count, self.limit
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(CYC_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }

    fn add(&mut self) {
        if let Some(top) = self.stack.last_mut() {
            *top += 1;
        }
    }
}

impl<'a> Visit<'a> for CycVisitor<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        self.enter();
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.exit(name, node.span.start, node.span.end);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.enter();
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.exit("arrow function".to_string(), node.span.start, node.span.end);
    }

    fn visit_if_statement(&mut self, node: &IfStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_if_statement(self, node);
    }

    fn visit_for_statement(&mut self, node: &ForStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_for_statement(self, node);
    }

    fn visit_for_in_statement(&mut self, node: &ForInStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_for_in_statement(self, node);
    }

    fn visit_for_of_statement(&mut self, node: &ForOfStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_for_of_statement(self, node);
    }

    fn visit_while_statement(&mut self, node: &WhileStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_while_statement(self, node);
    }

    fn visit_do_while_statement(&mut self, node: &DoWhileStatement<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_do_while_statement(self, node);
    }

    fn visit_switch_statement(&mut self, node: &SwitchStatement<'a>) {
        // McCabe: +1 per *non-default* case. `default` is a fallthrough,
        // not an independent path.
        for case in &node.cases {
            if case.test.is_some() {
                self.add();
            }
        }
        oxc_ast_visit::walk::walk_switch_statement(self, node);
    }

    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
        // && || ?? all introduce short-circuit branches.
        self.add();
        oxc_ast_visit::walk::walk_logical_expression(self, node);
    }

    fn visit_conditional_expression(&mut self, node: &ConditionalExpression<'a>) {
        self.add();
        oxc_ast_visit::walk::walk_conditional_expression(self, node);
    }

    fn visit_try_statement(&mut self, node: &TryStatement<'a>) {
        if node.handler.is_some() {
            self.add();
        }
        oxc_ast_visit::walk::walk_try_statement(self, node);
    }
}
