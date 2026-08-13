use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, LineIndex, Location, OptionDefault,
    OptionKind, OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, ConditionalExpression, DoWhileStatement, ForInStatement,
    ForOfStatement, ForStatement, Function, FunctionBody, IfStatement, LogicalExpression,
    SwitchStatement, TryStatement, WhileStatement,
};
use oxc_ast_visit::Visit;

// ─── Refactor.LongAndComplex ───────────────────────────────────────────────

/// `Refactor.LongAndComplex` — flag functions that are both long AND
/// cyclomatically complex. Higher-confidence refactor signal than either
/// dimension alone; either alone produces a long tail of false positives
/// (flat config tables for length, deeply-branching short helpers for
/// complexity). The intersection is much narrower and almost always a
/// real candidate.
///
/// Length is body lines minus blanks and pure-comment lines. Cyclomatic
/// count is one plus each branch, loop, `case`, `catch` and short-circuit
/// operator.
pub struct LongAndComplex {
    length_limit: u32,
    cyclomatic_limit: u32,
}

impl LongAndComplex {
    /// Construct with paired length and cyclomatic ceilings. The check
    /// fires only when a function exceeds BOTH — neither alone is
    /// enough. `all_builtins` installs (75, 15).
    pub fn new(length_limit: u32, cyclomatic_limit: u32) -> Self {
        Self {
            length_limit,
            cyclomatic_limit,
        }
    }
}

const LAC_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "length_limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(75),
        doc: "minimum non-blank-non-comment body lines for a function to be considered long",
    },
    OptionSpec {
        name: "cyclomatic_limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(15),
        doc: "minimum cyclomatic complexity for a function to be considered complex",
    },
];

const LAC_META: CheckMeta = CheckMeta {
    id: "Refactor.LongAndComplex",
    category: Category::Refactor,
    base_priority: 12,
    default_severity: Severity::High,
    explanation: "Functions that are both long and complex are the strongest refactor candidates. Length alone catches flat config tables; complexity alone catches deeply-branching short helpers. The intersection is almost always a real refactor target.",
    body: include_str!("../../docs/Refactor.LongAndComplex.md"),
    requires_types: false,
    consistency: false,
    options: LAC_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl Check for LongAndComplex {
    fn meta(&self) -> &'static CheckMeta {
        &LAC_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let length_limit = ctx
            .options
            .get_int("length_limit")
            .map(|v| v as u32)
            .unwrap_or(self.length_limit);
        let cyclomatic_limit = ctx
            .options
            .get_int("cyclomatic_limit")
            .map(|v| v as u32)
            .unwrap_or(self.cyclomatic_limit);
        let line_views: Vec<cofferdam_core::LineView<'_>> =
            cofferdam_core::Lines::build(&file.text, parsed.program).collect();
        let line_index = LineIndex::new(&file.text);
        let mut visitor = LACVisitor {
            file,
            length_limit,
            cyclomatic_limit,
            line_views: &line_views,
            line_index: &line_index,
            issues: Vec::new(),
            stack: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct LACFrame {
    name: String,
    body_span_start: u32,
    body_span_end: u32,
    cyc: u32,
    /// `true` when this frame represents a function with no body
    /// (declaration only — TS overload signatures, ambient declarations).
    /// `exit` returns early without emitting.
    is_sentinel: bool,
}

struct LACVisitor<'a> {
    file: &'a SourceFile,
    length_limit: u32,
    cyclomatic_limit: u32,
    line_views: &'a [cofferdam_core::LineView<'a>],
    line_index: &'a LineIndex,
    issues: Vec<Issue>,
    stack: Vec<LACFrame>,
}

impl<'a> LACVisitor<'a> {
    fn enter_with_body(&mut self, name: String, body: &FunctionBody<'_>) {
        self.stack.push(LACFrame {
            name,
            body_span_start: body.span.start,
            body_span_end: body.span.end,
            cyc: 1,
            is_sentinel: false,
        });
    }

    fn enter_sentinel(&mut self, name: String) {
        self.stack.push(LACFrame {
            name,
            body_span_start: 0,
            body_span_end: 0,
            cyc: 1,
            is_sentinel: true,
        });
    }

    fn exit(&mut self) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        if frame.is_sentinel {
            return;
        }

        let start_line = self
            .line_index
            .span_from_bytes(frame.body_span_start, frame.body_span_start)
            .line;
        let end_line = self
            .line_index
            .span_from_bytes(frame.body_span_end, frame.body_span_end)
            .line;
        let raw = end_line.saturating_sub(start_line);
        let inner_lo = start_line.saturating_add(1);
        let inner_hi = end_line.saturating_sub(1);
        let skipped = crate::count_skippable_lines(self.line_views, inner_lo, inner_hi);
        let length = raw.saturating_sub(skipped);

        if length > self.length_limit && frame.cyc > self.cyclomatic_limit {
            let span = self
                .line_index
                .span_from_bytes(frame.body_span_start, frame.body_span_end);
            self.issues.push(Issue {
                check_id: LAC_META.id.to_string(),
                message: format!(
                    "{} is long ({} lines) AND complex (cyclomatic {}) — exceeds {}/{}",
                    frame.name, length, frame.cyc, self.length_limit, self.cyclomatic_limit
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(LAC_META.base_priority),
                severity: Severity::High,
                related: Vec::new(),
            });
        }
    }

    fn add(&mut self) {
        if let Some(top) = self.stack.last_mut() {
            top.cyc += 1;
        }
    }
}

impl<'a> Visit<'a> for LACVisitor<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        if let Some(body) = &node.body {
            self.enter_with_body(name, body);
        } else {
            self.enter_sentinel(name);
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.exit();
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        // Skip expression-bodied arrows: too short to be the target of
        // this check, same as `Readability.MaxFunctionLength`.
        if node.expression {
            self.enter_sentinel("arrow function".to_string());
            oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
            self.exit();
            return;
        }
        self.enter_with_body("arrow function".to_string(), &node.body);
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.exit();
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
        for case in &node.cases {
            if case.test.is_some() {
                self.add();
            }
        }
        oxc_ast_visit::walk::walk_switch_statement(self, node);
    }

    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
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
