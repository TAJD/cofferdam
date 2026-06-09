use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, OptionDefault, OptionKind,
    OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, CallExpression, ConditionalExpression, DoWhileStatement, Expression,
    ForInStatement, ForOfStatement, ForStatement, Function, IfStatement, LogicalExpression,
    LogicalOperator, Statement, SwitchStatement, TryStatement, WhileStatement,
};
use oxc_ast_visit::Visit;

// ─── Refactor.CognitiveComplexity ──────────────────────────────────────────

/// `Refactor.CognitiveComplexity` — Sonar-style score per function.
///
/// Sonar cognitive complexity: structural breaks (`if`, loops, `switch`, `catch`,
/// ternary) cost `1 + nesting`; logical operators follow Sonar's B3 mixed-operator
/// rule (a sequence of same-kind operators counts as `1` flat; switching kind adds `1`
/// more). `else if` chains do not stack additional nesting. Plain `else` costs `1`
/// flat (no nesting penalty). Direct recursive calls (function calling itself by name)
/// cost `1` flat.
pub struct CognitiveComplexity {
    limit: u32,
}

impl CognitiveComplexity {
    /// Construct with a per-function cognitive-complexity ceiling.
    /// `all_builtins` installs the default of 15; user config overrides
    /// via `[checks."Refactor.CognitiveComplexity"].limit`.
    pub fn new(limit: u32) -> Self {
        Self { limit }
    }
}

const COG_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(15),
    doc: "maximum cognitive complexity per function",
}];

const COG_META: CheckMeta = CheckMeta {
    id: "Refactor.CognitiveComplexity",
    category: Category::Refactor,
    base_priority: 10,
    default_severity: Severity::Medium,
    explanation: "Sonar-style cognitive complexity. Branching breaks plus a nesting penalty — deeply nested code costs more than a long flat switch.",
    body: include_str!("../../docs/Refactor.CognitiveComplexity.md"),
    requires_types: false,
    consistency: false,
    options: COG_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl Check for CognitiveComplexity {
    fn meta(&self) -> &'static CheckMeta {
        &COG_META
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
        let mut visitor = CogVisitor {
            file,
            limit,
            issues: Vec::new(),
            stack: Vec::new(),
            name_stack: Vec::new(),
            nesting: 0,
            logical_op_stack: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct CogVisitor<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
    /// Per-function running total. Same lifecycle as CycVisitor.stack.
    stack: Vec<u32>,
    /// Parallel stack tracking the name of each function being visited.
    /// `None` for anonymous/arrow functions where named recursion doesn't apply.
    /// Paired with `stack` — must push/pop in sync.
    name_stack: Vec<Option<String>>,
    /// Nesting depth inside the current function. Reset on function entry
    /// (nested function bodies start fresh — Sonar treats them as new
    /// units).
    nesting: u32,
    /// Stack of logical operators for B3 mixed-operator rule. Tracks the
    /// operator of each enclosing LogicalExpression to detect when we switch
    /// from one operator type to another (e.g., && to || to &&). Reset on
    /// function entry.
    logical_op_stack: Vec<LogicalOperator>,
}

impl<'a> CogVisitor<'a> {
    fn enter(&mut self, name: Option<String>) {
        self.stack.push(0);
        self.name_stack.push(name);
        self.nesting = 0;
        self.logical_op_stack.clear();
    }

    fn exit(&mut self, name: String, span_start: u32, span_end: u32) {
        let count = self.stack.pop().unwrap_or(0);
        let _ = self.name_stack.pop(); // Must stay in sync with stack
        if count > self.limit {
            let span = span_from_bytes(&self.file.text, span_start, span_end);
            self.issues.push(Issue {
                check_id: COG_META.id.to_string(),
                message: format!(
                    "{} has cognitive complexity {}, exceeds limit of {}",
                    name, count, self.limit
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(COG_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }

    /// Structural cost: +1 for the keyword + the current nesting penalty.
    fn structural(&mut self) {
        let add = 1 + self.nesting;
        if let Some(top) = self.stack.last_mut() {
            *top += add;
        }
    }

    /// Flat cost: +1, no nesting penalty (e.g. `&&` / `||`).
    fn flat(&mut self) {
        if let Some(top) = self.stack.last_mut() {
            *top += 1;
        }
    }
}

impl<'a> Visit<'a> for CogVisitor<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let func_name = node.id.as_ref().map(|id| id.name.as_str().to_string());
        let display_name = func_name
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "anonymous function".to_string());
        // Save & restore nesting and logical_op_stack so nested function bodies start fresh.
        let saved_nesting = self.nesting;
        let saved_logical_op_stack = self.logical_op_stack.clone();
        self.enter(func_name);
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.exit(display_name, node.span.start, node.span.end);
        self.nesting = saved_nesting;
        self.logical_op_stack = saved_logical_op_stack;
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        let saved_nesting = self.nesting;
        let saved_logical_op_stack = self.logical_op_stack.clone();
        self.enter(None); // Arrow functions don't have names; recursion-by-name not applicable
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.exit("arrow function".to_string(), node.span.start, node.span.end);
        self.nesting = saved_nesting;
        self.logical_op_stack = saved_logical_op_stack;
    }

    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        // Detect recursive self-call: function calling itself by name.
        // Only bare-identifier calls count; method calls (obj.foo()) don't.
        if let Expression::Identifier(ident) = &node.callee {
            if let Some(Some(current)) = self.name_stack.last() {
                if ident.name.as_str() == current {
                    self.flat(); // +1 flat, no nesting penalty
                }
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_if_statement(&mut self, node: &IfStatement<'a>) {
        self.structural();
        // test runs at the if's own nesting (no +1)
        self.visit_expression(&node.test);
        // consequent body is one level deeper
        self.nesting += 1;
        self.visit_statement(&node.consequent);
        self.nesting -= 1;
        // alternate handling: `else if` (alternate is another IfStatement)
        // does NOT stack a nesting penalty — Sonar treats the chain as
        // sibling structural breaks. A plain `else { ... }` block adds
        // +1 flat (no nesting penalty), then walks at +1 nesting depth.
        if let Some(alt) = &node.alternate {
            match alt {
                Statement::IfStatement(inner) => self.visit_if_statement(inner),
                other => {
                    self.flat();
                    self.nesting += 1;
                    self.visit_statement(other);
                    self.nesting -= 1;
                }
            }
        }
    }

    fn visit_for_statement(&mut self, node: &ForStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_for_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_for_in_statement(&mut self, node: &ForInStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_for_in_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_for_of_statement(&mut self, node: &ForOfStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_for_of_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_while_statement(&mut self, node: &WhileStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_while_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_do_while_statement(&mut self, node: &DoWhileStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_do_while_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_switch_statement(&mut self, node: &SwitchStatement<'a>) {
        self.structural();
        self.nesting += 1;
        oxc_ast_visit::walk::walk_switch_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_try_statement(&mut self, node: &TryStatement<'a>) {
        if node.handler.is_some() {
            self.structural();
        }
        self.nesting += 1;
        oxc_ast_visit::walk::walk_try_statement(self, node);
        self.nesting -= 1;
    }

    fn visit_conditional_expression(&mut self, node: &ConditionalExpression<'a>) {
        self.structural();
        // Ternary branches are sub-expressions, not statements — Sonar
        // counts the `?:` itself but not extra nesting for the arms.
        oxc_ast_visit::walk::walk_conditional_expression(self, node);
    }

    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
        // B3 mixed-operator rule: a sequence of same-kind operators costs +1 total;
        // switching kind adds another +1. Stack tracks the operator of the current
        // chain. If we pop and the top differs from this node's operator, or if
        // the stack is empty, this is a NEW segment.
        let new_segment = self
            .logical_op_stack
            .last()
            .copied()
            .map(|prev| prev != node.operator)
            .unwrap_or(true);
        if new_segment {
            self.flat();
        }
        self.logical_op_stack.push(node.operator);
        oxc_ast_visit::walk::walk_logical_expression(self, node);
        self.logical_op_stack.pop();
    }
}
