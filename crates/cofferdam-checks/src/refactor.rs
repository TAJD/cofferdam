//! Refactor checks — mechanical cleanups, often autofixable.
//!
//! Cyclomatic and cognitive complexity both walk function-like nodes
//! and tally a per-function score. They differ only in scoring rules:
//! McCabe cyclomatic counts independent paths flatly; Sonar cognitive
//! adds a nesting penalty so deeply-nested branching costs more than
//! a long flat switch.
//!
//! Both checks ignore code outside any function (top-level statements
//! at module scope) — the metrics are designed for callable units.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Priority,
    RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, BinaryExpression, BindingIdentifier,
    BlockStatement, BooleanLiteral, BreakStatement, CallExpression, Class, ConditionalExpression,
    ContinueStatement, DoWhileStatement, Expression, ExpressionStatement, ForInStatement,
    ForOfStatement, ForStatement, Function, FunctionBody, IdentifierName, IdentifierReference,
    IfStatement, LogicalExpression, LogicalOperator, NewExpression, NullLiteral, NumericLiteral,
    Program, ReturnStatement, Statement, StringLiteral, SwitchStatement, TemplateLiteral,
    ThrowStatement, TryStatement, UnaryExpression, UpdateExpression, VariableDeclaration,
    WhileStatement,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

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
    pub fn new(limit: u32) -> Self {
        Self { limit }
    }
}

const CYC_META: CheckMeta = CheckMeta {
    id: "Refactor.CyclomaticComplexity",
    category: Category::Refactor,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "McCabe cyclomatic complexity counts independent paths through a function. High values indicate branching that's hard to test and reason about.",
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for CyclomaticComplexity {
    fn meta(&self) -> &'static CheckMeta {
        &CYC_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = CycVisitor {
            file,
            limit: self.limit,
            issues: Vec::new(),
            stack: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct CycVisitor<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
    /// Per-function tally. Push 1 (McCabe's base) on entry, pop on exit.
    /// Nested functions get their own entry — outer function's tally is
    /// undisturbed by inner branching.
    stack: Vec<u32>,
}

impl<'a> CycVisitor<'a> {
    fn enter(&mut self) {
        self.stack.push(1);
    }

    fn exit(&mut self, name: String, span_start: u32, span_end: u32) {
        let count = self.stack.pop().unwrap_or(1);
        if count > self.limit {
            let span = span_from_bytes(&self.file.text, span_start, span_end);
            self.issues.push(Issue {
                check_id: CYC_META.id.to_string(),
                message: format!(
                    "{} has cyclomatic complexity {}, exceeds limit of {}",
                    name, count, self.limit
                ),
                file: self.file.path.clone(),
                span,
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
    pub fn new(limit: u32) -> Self {
        Self { limit }
    }
}

const COG_META: CheckMeta = CheckMeta {
    id: "Refactor.CognitiveComplexity",
    category: Category::Refactor,
    base_priority: 10,
    default_severity: Severity::Medium,
    explanation: "Sonar-style cognitive complexity. Branching breaks plus a nesting penalty — deeply nested code costs more than a long flat switch.",
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for CognitiveComplexity {
    fn meta(&self) -> &'static CheckMeta {
        &COG_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = CogVisitor {
            file,
            limit: self.limit,
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
                span,
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

// ─── Refactor.DuplicateBlock ───────────────────────────────────────────────
//
// Cross-file check. Detects copy-paste — runs of `min_statements`
// consecutive statements that match (after rename canonicalisation) in
// two or more files. Uses the cd-0ps corpus API: per-file `run` collects
// fingerprints into a shared slot; `finalize` groups by hash and emits
// one Issue per duplicate set, with the canonical occurrence as the
// primary span and the rest as `Issue.related`.
//
// v1 scope (cd-qnu AST mode):
//   - Sliding window over statement runs in `Program.body` and every
//     `BlockStatement.body`.
//   - Canonicalisation is source-text level: identifier tokens get
//     mapped to per-window local indices (`$_0`, `$_1` ...); keywords
//     and literals stay verbatim; comments and whitespace are stripped/
//     collapsed. ASCII identifier scan only — Unicode identifiers are
//     a v2 follow-up.
//   - Overlapping windows in the same file are deduped at finalize:
//     the first emitted finding "claims" its span, later overlapping
//     windows are dropped. Sufficient to keep a 10-statement duplicate
//     from producing 5 redundant issues.
//
// Out of scope at v1 (separate beads):
//   - Token-mode scanning (sliding window over the lexer's token stream
//     regardless of statement boundaries) — cd-qnu's design step 4.
//   - Config wiring via cofferdam.toml — cd-4ms.
//   - True AST-subtree canonical hashing (rather than text-level) for
//     resilience to whitespace + comment placement edge cases.

const DUPLICATE_BLOCK_MIN_STATEMENTS: usize = 6;
/// Sanity floor: a window covering fewer chars than this is too small
/// to be a meaningful duplicate (e.g., six one-liners). Filters out
/// trivial runs of `import` / `export` / `const X;`.
const DUPLICATE_BLOCK_MIN_CHARS: usize = 80;

/// One canonicalised window, recorded during `Check::run`. Read back
/// in `finalize` to find duplicates. `kind` distinguishes AST-mode hits
/// (statement-aligned, default) from token-mode hits (sliding token
/// windows that may cross statement boundaries) so finalize can prefer
/// the former when they overlap.
#[derive(Clone)]
struct Fingerprint {
    hash: u64,
    kind: FingerprintKind,
    file: PathBuf,
    span: Span,
}

/// Order matters: AST first so finalize's overlap-dedupe pass claims
/// statement-aligned territory before token-mode fragments compete.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FingerprintKind {
    Ast,
    Token,
}

static DUPLICATE_BLOCKS: CorpusKey<Vec<Fingerprint>> =
    CorpusKey::new("Refactor.DuplicateBlock.fingerprints");

/// Sliding window size for token mode. Defaults to ~50 tokens —
/// roughly 3-5 lines of typical TS once operators and punctuation
/// are counted as separate tokens.
const DUPLICATE_BLOCK_MIN_TOKENS: usize = 50;

pub struct DuplicateBlock {
    min_statements: usize,
    min_chars: usize,
    /// Token-mode min window size. Only used when `include_tokens`.
    min_tokens: usize,
    /// Opt-in: also emit findings from a sliding token-window pass.
    /// Off by default — duplicates the work of AST mode for most
    /// hits, only paying off where copy-paste spans non-statement
    /// boundaries (a multi-line conditional broken across statements
    /// differently in two places, JSX runs, etc.).
    include_tokens: bool,
}

impl Default for DuplicateBlock {
    fn default() -> Self {
        Self {
            min_statements: DUPLICATE_BLOCK_MIN_STATEMENTS,
            min_chars: DUPLICATE_BLOCK_MIN_CHARS,
            min_tokens: DUPLICATE_BLOCK_MIN_TOKENS,
            include_tokens: false,
        }
    }
}

impl DuplicateBlock {
    /// Construct with token-mode enabled and the given window size.
    /// AST mode stays on at default thresholds. The two modes share
    /// one corpus slot; finalize dedupes overlapping spans, preferring
    /// AST hits.
    pub fn with_tokens(min_tokens: usize) -> Self {
        Self {
            include_tokens: true,
            min_tokens,
            ..Self::default()
        }
    }
}

const DUP_META: CheckMeta = CheckMeta {
    id: "Refactor.DuplicateBlock",
    category: Category::Refactor,
    base_priority: 12,
    default_severity: Severity::Medium,
    explanation: "Runs of statements that recur (after rename canonicalisation) in multiple files. Likely copy-paste — extract a shared helper.",
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for DuplicateBlock {
    fn meta(&self) -> &'static CheckMeta {
        &DUP_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = DupCollector {
            file,
            min_statements: self.min_statements,
            min_chars: self.min_chars,
            collected: Vec::new(),
        };
        visitor.visit_program(parsed.program);

        if self.include_tokens {
            collect_token_fingerprints(
                file,
                self.min_tokens,
                self.min_chars,
                &mut visitor.collected,
            );
        }

        ctx.corpus.with_slot(&DUPLICATE_BLOCKS, |slot| {
            slot.append(&mut visitor.collected);
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let mut by_hash: BTreeMap<u64, Vec<Fingerprint>> = BTreeMap::new();
        ctx.corpus.with_slot(&DUPLICATE_BLOCKS, |slot| {
            for fp in slot.drain(..) {
                by_hash.entry(fp.hash).or_default().push(fp);
            }
        });

        // Build candidate findings and dedupe overlapping windows in the
        // same file. Sort groups by their primary's (file, start_byte) so
        // earlier-in-the-source windows claim the territory first.
        let mut candidates: Vec<Vec<Fingerprint>> = by_hash
            .into_values()
            .filter(|fps| fps.len() >= 2)
            .map(|mut fps| {
                fps.sort_by(|a, b| {
                    a.file
                        .cmp(&b.file)
                        .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
                });
                fps
            })
            .collect();
        // AST candidates run first so they claim territory before token
        // candidates compete. Inside a kind, sort by primary's location.
        candidates.sort_by(|a, b| {
            a[0].kind
                .cmp(&b[0].kind)
                .then_with(|| a[0].file.cmp(&b[0].file))
                .then_with(|| a[0].span.start_byte.cmp(&b[0].span.start_byte))
        });

        // (file, start, end) of every primary span we've already emitted.
        // A new candidate whose primary OR any related span overlaps an
        // already-emitted region in the same file is dropped.
        let mut claimed: Vec<(PathBuf, u32, u32)> = Vec::new();
        let mut issues = Vec::new();

        for group in candidates {
            let primary = &group[0];
            let overlaps = |c: &(PathBuf, u32, u32), file: &PathBuf, s: u32, e: u32| {
                &c.0 == file && c.1 < e && s < c.2
            };
            if claimed.iter().any(|c| {
                overlaps(
                    c,
                    &primary.file,
                    primary.span.start_byte,
                    primary.span.end_byte,
                )
            }) {
                continue;
            }
            // Also drop if any related span overlaps an already-claimed
            // region — same logical duplicate, viewed from the other side.
            if group[1..].iter().any(|fp| {
                claimed
                    .iter()
                    .any(|c| overlaps(c, &fp.file, fp.span.start_byte, fp.span.end_byte))
            }) {
                continue;
            }

            for fp in &group {
                claimed.push((fp.file.clone(), fp.span.start_byte, fp.span.end_byte));
            }

            let related: Vec<RelatedSpan> = group[1..]
                .iter()
                .map(|fp| RelatedSpan {
                    file: fp.file.clone(),
                    span: fp.span,
                })
                .collect();
            let message = match primary.kind {
                FingerprintKind::Ast => format!(
                    "duplicate {}-statement block, also at {} other location(s)",
                    self.min_statements,
                    related.len()
                ),
                FingerprintKind::Token => format!(
                    "duplicate {}-token window (cross-statement), also at {} other location(s)",
                    self.min_tokens,
                    related.len()
                ),
            };
            issues.push(Issue {
                check_id: DUP_META.id.to_string(),
                message,
                file: primary.file.clone(),
                span: primary.span,
                priority: Priority(DUP_META.base_priority),
                severity: Severity::Medium,
                related,
            });
        }
        issues
    }
}

struct DupCollector<'a> {
    file: &'a SourceFile,
    min_statements: usize,
    min_chars: usize,
    collected: Vec<Fingerprint>,
}

impl<'a> DupCollector<'a> {
    fn scan(&mut self, stmts: &[Statement<'a>]) {
        if stmts.len() < self.min_statements {
            return;
        }
        for i in 0..=stmts.len() - self.min_statements {
            let first = &stmts[i];
            let last = &stmts[i + self.min_statements - 1];
            let start = first.span().start as usize;
            let end = last.span().end as usize;
            if start >= end || end > self.file.text.len() {
                continue;
            }
            // The min_chars floor still uses raw byte length — cheaper
            // than walking the AST just to discard a tiny window.
            if (end - start) < self.min_chars {
                continue;
            }
            let window = &stmts[i..i + self.min_statements];
            let hash = hash_ast_window(window);
            let span = span_from_bytes(&self.file.text, start as u32, end as u32);
            self.collected.push(Fingerprint {
                hash,
                kind: FingerprintKind::Ast,
                file: self.file.path.clone(),
                span,
            });
        }
    }
}

impl<'a> Visit<'a> for DupCollector<'a> {
    fn visit_program(&mut self, node: &Program<'a>) {
        self.scan(&node.body);
        oxc_ast_visit::walk::walk_program(self, node);
    }

    fn visit_block_statement(&mut self, node: &BlockStatement<'a>) {
        self.scan(&node.body);
        oxc_ast_visit::walk::walk_block_statement(self, node);
    }

    fn visit_function_body(&mut self, node: &FunctionBody<'a>) {
        // Function and arrow-function bodies are FunctionBody, NOT
        // BlockStatement, in oxc. Scan their `statements` directly so
        // duplicates inside ordinary function bodies are caught.
        self.scan(&node.statements);
        oxc_ast_visit::walk::walk_function_body(self, node);
    }
}

/// JS/TS reserved + common type-position keywords. Words in this set
/// stay verbatim during canonicalisation — substituting them would
/// destroy structural meaning (`let x = 1` ≠ `var x = 1`).
const JS_KEYWORDS: &[&str] = &[
    "abstract",
    "any",
    "as",
    "async",
    "await",
    "boolean",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "constructor",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "infer",
    "instanceof",
    "interface",
    "is",
    "keyof",
    "let",
    "module",
    "namespace",
    "never",
    "new",
    "null",
    "number",
    "object",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "return",
    "set",
    "static",
    "string",
    "super",
    "switch",
    "symbol",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "unique",
    "unknown",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn is_keyword(word: &str) -> bool {
    JS_KEYWORDS.binary_search(&word).is_ok()
}

/// Identifier-start predicate. JS/TS adds `_` and `$` on top of
/// Unicode XID_Start, so we test those explicitly.
fn is_ident_start(c: char) -> bool {
    c == '_' || c == '$' || unicode_ident::is_xid_start(c)
}

/// Identifier-continue predicate. JS/TS adds `_` and `$` on top of
/// Unicode XID_Continue. ZWNJ/ZWJ are technically allowed by the JS
/// spec but we don't support them — matching parity with oxc's lexer
/// which also treats them as joiner-only edge cases.
fn is_ident_continue(c: char) -> bool {
    c == '_' || c == '$' || unicode_ident::is_xid_continue(c)
}

// ─── AST canonical hashing (cd-mti) ────────────────────────────────────────
//
// Walks an AST subtree (or, in DuplicateBlock's case, a statement run) and
// folds a structural hash that's resilient to:
//   - Identifier renames     (mapped to per-window `$_N` indices)
//   - Whitespace + comments  (the AST has neither)
//   - Brace style / block vs single-statement bodies (different AST shapes,
//     so different hashes — the text canonicaliser collapsed both forms)
//
// Each visit_X method tags the hash with a short structural identifier
// before walking children. Identifier references contribute their local
// index; literals contribute their value (literal-sensitive at v1, same
// as the text canonicaliser).
//
// v1 covers the common JS/TS shapes — TS-specific nodes (decorators,
// type annotations, etc.) walk through without a tag, which means two
// statements that differ ONLY in those nodes can hash equal. Acceptable
// trade-off; cd-mti follow-ups can extend the visitor.

struct AstHashWalker {
    hasher: std::collections::hash_map::DefaultHasher,
    locals: HashMap<String, u32>,
    next_local: u32,
}

impl AstHashWalker {
    fn new() -> Self {
        Self {
            hasher: std::collections::hash_map::DefaultHasher::new(),
            locals: HashMap::new(),
            next_local: 0,
        }
    }

    fn tag(&mut self, bytes: &[u8]) {
        use std::hash::Hasher;
        self.hasher.write(bytes);
        self.hasher.write_u8(0x01);
    }

    fn ident_index(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.locals.get(name) {
            return i;
        }
        let i = self.next_local;
        self.next_local += 1;
        self.locals.insert(name.to_string(), i);
        i
    }

    fn ident_tag(&mut self, prefix: &[u8], name: &str) {
        use std::hash::Hasher;
        let i = self.ident_index(name);
        self.hasher.write(prefix);
        self.hasher.write_u32(i);
        self.hasher.write_u8(0x01);
    }

    fn finish(self) -> u64 {
        use std::hash::Hasher;
        self.hasher.finish()
    }
}

impl<'a> Visit<'a> for AstHashWalker {
    fn visit_block_statement(&mut self, node: &BlockStatement<'a>) {
        self.tag(b"Blk");
        oxc_ast_visit::walk::walk_block_statement(self, node);
    }

    fn visit_expression_statement(&mut self, node: &ExpressionStatement<'a>) {
        self.tag(b"ExpS");
        oxc_ast_visit::walk::walk_expression_statement(self, node);
    }

    fn visit_if_statement(&mut self, node: &IfStatement<'a>) {
        self.tag(b"If");
        if node.alternate.is_some() {
            self.tag(b"+E");
        }
        oxc_ast_visit::walk::walk_if_statement(self, node);
    }

    fn visit_for_statement(&mut self, node: &ForStatement<'a>) {
        self.tag(b"For");
        oxc_ast_visit::walk::walk_for_statement(self, node);
    }

    fn visit_for_in_statement(&mut self, node: &ForInStatement<'a>) {
        self.tag(b"ForIn");
        oxc_ast_visit::walk::walk_for_in_statement(self, node);
    }

    fn visit_for_of_statement(&mut self, node: &ForOfStatement<'a>) {
        self.tag(b"ForOf");
        oxc_ast_visit::walk::walk_for_of_statement(self, node);
    }

    fn visit_while_statement(&mut self, node: &WhileStatement<'a>) {
        self.tag(b"Whl");
        oxc_ast_visit::walk::walk_while_statement(self, node);
    }

    fn visit_do_while_statement(&mut self, node: &DoWhileStatement<'a>) {
        self.tag(b"Do");
        oxc_ast_visit::walk::walk_do_while_statement(self, node);
    }

    fn visit_switch_statement(&mut self, node: &SwitchStatement<'a>) {
        self.tag(b"Sw");
        oxc_ast_visit::walk::walk_switch_statement(self, node);
    }

    fn visit_try_statement(&mut self, node: &TryStatement<'a>) {
        self.tag(b"Try");
        if node.handler.is_some() {
            self.tag(b"+C");
        }
        if node.finalizer.is_some() {
            self.tag(b"+F");
        }
        oxc_ast_visit::walk::walk_try_statement(self, node);
    }

    fn visit_return_statement(&mut self, node: &ReturnStatement<'a>) {
        self.tag(b"Ret");
        oxc_ast_visit::walk::walk_return_statement(self, node);
    }

    fn visit_throw_statement(&mut self, node: &ThrowStatement<'a>) {
        self.tag(b"Thr");
        oxc_ast_visit::walk::walk_throw_statement(self, node);
    }

    fn visit_break_statement(&mut self, node: &BreakStatement<'a>) {
        self.tag(b"Brk");
        oxc_ast_visit::walk::walk_break_statement(self, node);
    }

    fn visit_continue_statement(&mut self, node: &ContinueStatement<'a>) {
        self.tag(b"Cnt");
        oxc_ast_visit::walk::walk_continue_statement(self, node);
    }

    fn visit_variable_declaration(&mut self, node: &VariableDeclaration<'a>) {
        self.tag(b"Var:");
        self.tag(node.kind.as_str().as_bytes());
        oxc_ast_visit::walk::walk_variable_declaration(self, node);
    }

    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        self.tag(b"Fn");
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.tag(b"Arrow");
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }

    fn visit_class(&mut self, node: &Class<'a>) {
        self.tag(b"Cls");
        oxc_ast_visit::walk::walk_class(self, node);
    }

    fn visit_binary_expression(&mut self, node: &BinaryExpression<'a>) {
        self.tag(b"Bin:");
        self.tag(node.operator.as_str().as_bytes());
        oxc_ast_visit::walk::walk_binary_expression(self, node);
    }

    fn visit_logical_expression(&mut self, node: &LogicalExpression<'a>) {
        self.tag(b"Log:");
        self.tag(node.operator.as_str().as_bytes());
        oxc_ast_visit::walk::walk_logical_expression(self, node);
    }

    fn visit_unary_expression(&mut self, node: &UnaryExpression<'a>) {
        self.tag(b"Una:");
        self.tag(node.operator.as_str().as_bytes());
        oxc_ast_visit::walk::walk_unary_expression(self, node);
    }

    fn visit_update_expression(&mut self, node: &UpdateExpression<'a>) {
        self.tag(b"Upd:");
        self.tag(node.operator.as_str().as_bytes());
        self.tag(if node.prefix { b"P" } else { b"S" });
        oxc_ast_visit::walk::walk_update_expression(self, node);
    }

    fn visit_assignment_expression(&mut self, node: &AssignmentExpression<'a>) {
        self.tag(b"Asn:");
        self.tag(node.operator.as_str().as_bytes());
        oxc_ast_visit::walk::walk_assignment_expression(self, node);
    }

    fn visit_conditional_expression(&mut self, node: &ConditionalExpression<'a>) {
        self.tag(b"Tern");
        oxc_ast_visit::walk::walk_conditional_expression(self, node);
    }

    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        self.tag(b"Call");
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_new_expression(&mut self, node: &NewExpression<'a>) {
        self.tag(b"New");
        oxc_ast_visit::walk::walk_new_expression(self, node);
    }

    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.ident_tag(b"Id:", ident.name.as_str());
    }

    fn visit_binding_identifier(&mut self, ident: &BindingIdentifier<'a>) {
        self.ident_tag(b"Bid:", ident.name.as_str());
    }

    fn visit_identifier_name(&mut self, ident: &IdentifierName<'a>) {
        // Static member names + property keys flow here. Hash by index too
        // so `obj.foo` ≠ `obj.bar` but renames of foo across files match.
        self.ident_tag(b"Idn:", ident.name.as_str());
    }

    fn visit_string_literal(&mut self, lit: &StringLiteral<'a>) {
        use std::hash::Hasher;
        self.hasher.write(b"Str:");
        self.hasher.write(lit.value.as_bytes());
        self.hasher.write_u8(0x01);
    }

    fn visit_numeric_literal(&mut self, lit: &NumericLiteral<'a>) {
        use std::hash::Hasher;
        self.hasher.write(b"Num:");
        self.hasher.write(&lit.value.to_le_bytes());
        self.hasher.write_u8(0x01);
    }

    fn visit_boolean_literal(&mut self, lit: &BooleanLiteral) {
        self.tag(if lit.value { b"T" } else { b"F" });
    }

    fn visit_null_literal(&mut self, _lit: &NullLiteral) {
        self.tag(b"Nul");
    }

    fn visit_template_literal(&mut self, node: &TemplateLiteral<'a>) {
        self.tag(b"Tmpl");
        oxc_ast_visit::walk::walk_template_literal(self, node);
    }
}

/// Hash a window of consecutive statements via AST canonicalisation.
/// Replaces the source-text + regex approach for AST-mode duplicate
/// detection (cd-mti). Token mode still uses text canonicalisation.
fn hash_ast_window<'a>(stmts: &[Statement<'a>]) -> u64 {
    let mut walker = AstHashWalker::new();
    for stmt in stmts {
        walker.visit_statement(stmt);
    }
    walker.finish()
}

// ─── Token mode (cd-jdq) ───────────────────────────────────────────────────

/// One canonicalised token plus its byte span in the original source.
/// Token mode slides a window over a Vec of these.
#[derive(Clone)]
struct TokenInfo {
    canon: String,
    start: u32,
    end: u32,
}

/// Tokenise the file's source into canonicalised tokens with spans.
///
/// Emits one token per:
/// - Identifier run (canonicalised to `$_N` per-file local index, or
///   kept verbatim if it's a JS/TS keyword).
/// - String literal (single/double/backtick, captured whole including
///   the quotes — escape handling is approximate for v1).
/// - Numeric literal (digits / `.` / `_`).
/// - Single non-identifier byte (operators, punctuation). `===` becomes
///   three single-character tokens; this means a `min_tokens` of 50
///   covers fewer source lines than you might expect.
///
/// Whitespace and comments are dropped entirely. ASCII identifier scan
/// only at v1 (cd-s2k tracks Unicode XID support).
fn tokenise(text: &str) -> Vec<TokenInfo> {
    let mut tokens = Vec::new();
    let mut locals: HashMap<String, u32> = HashMap::new();
    let mut next: u32 = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // Helper: byte offset of `chars[idx]`, or end of text if past the end.
    let byte_at = |chars: &[(usize, char)], idx: usize, text: &str| -> usize {
        if idx < chars.len() {
            chars[idx].0
        } else {
            text.len()
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if is_ident_start(c) {
            let start_byte = chars[i].0;
            i += 1;
            while i < chars.len() && is_ident_continue(chars[i].1) {
                i += 1;
            }
            let end_byte = byte_at(&chars, i, text);
            let word = &text[start_byte..end_byte];
            let canon = if is_keyword(word) {
                word.to_string()
            } else {
                let idx = match locals.get(word) {
                    Some(&v) => v,
                    None => {
                        let v = next;
                        next += 1;
                        locals.insert(word.to_string(), v);
                        v
                    }
                };
                format!("$_{}", idx)
            };
            tokens.push(TokenInfo {
                canon,
                start: start_byte as u32,
                end: end_byte as u32,
            });
        } else if c.is_ascii_whitespace() {
            while i < chars.len() && chars[i].1.is_ascii_whitespace() {
                i += 1;
            }
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1].1 == '/' {
            while i < chars.len() && chars[i].1 != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1].1 == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i].1 == '*' && chars[i + 1].1 == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start_byte = chars[i].0;
            i += 1;
            while i < chars.len() && chars[i].1 != quote {
                if chars[i].1 == '\\' && i + 1 < chars.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            // Consume the closing quote if present.
            if i < chars.len() {
                i += 1;
            }
            let end_byte = byte_at(&chars, i, text);
            tokens.push(TokenInfo {
                canon: text[start_byte..end_byte].to_string(),
                start: start_byte as u32,
                end: end_byte as u32,
            });
        } else if c.is_ascii_digit() {
            let start_byte = chars[i].0;
            while i < chars.len()
                && (chars[i].1.is_ascii_digit() || chars[i].1 == '.' || chars[i].1 == '_')
            {
                i += 1;
            }
            let end_byte = byte_at(&chars, i, text);
            tokens.push(TokenInfo {
                canon: text[start_byte..end_byte].to_string(),
                start: start_byte as u32,
                end: end_byte as u32,
            });
        } else {
            let start_byte = chars[i].0;
            let end_byte = if i + 1 < chars.len() {
                chars[i + 1].0
            } else {
                text.len()
            };
            tokens.push(TokenInfo {
                canon: c.to_string(),
                start: start_byte as u32,
                end: end_byte as u32,
            });
            i += 1;
        }
    }
    tokens
}

fn hash_token_window(window: &[TokenInfo]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let mut h = DefaultHasher::new();
    for t in window {
        h.write(t.canon.as_bytes());
        // Separator so adjacent tokens can't collide with a longer one
        // (e.g. `[` `]` should not hash like `[]`).
        h.write_u8(0x01);
    }
    h.finish()
}

fn collect_token_fingerprints(
    file: &SourceFile,
    min_tokens: usize,
    min_chars: usize,
    out: &mut Vec<Fingerprint>,
) {
    let tokens = tokenise(&file.text);
    if tokens.len() < min_tokens {
        return;
    }
    for i in 0..=tokens.len() - min_tokens {
        let window = &tokens[i..i + min_tokens];
        let start = window[0].start;
        let end = window[window.len() - 1].end;
        if start >= end || ((end - start) as usize) < min_chars {
            continue;
        }
        let hash = hash_token_window(window);
        let span = span_from_bytes(&file.text, start, end);
        out.push(Fingerprint {
            hash,
            kind: FingerprintKind::Token,
            file: file.path.clone(),
            span,
        });
    }
}

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

pub struct PreferOptionalChain;

const PREFER_OPTIONAL_CHAIN_META: CheckMeta = CheckMeta {
    id: "Refactor.PreferOptionalChain",
    category: Category::Refactor,
    base_priority: 5,
    default_severity: Severity::Low,
    explanation: "`a && a.b && a.b.c` is more concisely written as `a?.b?.c`. The optional-chain operator (`?.`) short-circuits on null/undefined.",
    requires_types: false,
    consistency: false,
    options: &[],
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
                            span,
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

pub struct PreferNullishCoalescing;

const PREFER_NULLISH_META: CheckMeta = CheckMeta {
    id: "Refactor.PreferNullishCoalescing",
    category: Category::Refactor,
    base_priority: 3,
    default_severity: Severity::Low,
    explanation: "`x || default` falls through on every falsy value (`0`, `\"\"`, `false`). Use `??` to fall through only on `null`/`undefined`.",
    requires_types: false,
    consistency: false,
    options: &[],
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
                span,
                priority: Priority(PREFER_NULLISH_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
        oxc_ast_visit::walk::walk_logical_expression(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenise_canonicalises_identifiers() {
        let toks = tokenise("const foo = bar + foo;");
        // const $_0 = $_1 + $_0 ;
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(canons, vec!["const", "$_0", "=", "$_1", "+", "$_0", ";"]);
    }

    #[test]
    fn tokenise_keeps_keywords_and_literals() {
        let toks = tokenise(r#"if (x === 5) return "hi";"#);
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["if", "(", "$_0", "=", "=", "=", "5", ")", "return", "\"hi\"", ";"]
        );
    }

    #[test]
    fn tokenise_strips_comments_and_whitespace() {
        let toks = tokenise("// leading\nconst x = 1; /* block */ const y = 2;");
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["const", "$_0", "=", "1", ";", "const", "$_1", "=", "2", ";"]
        );
    }

    #[test]
    fn rename_equivalent_windows_hash_equal() {
        let a = tokenise("const total = price + tax; return total;");
        let b = tokenise("const sum = amount + fee; return sum;");
        // Both canonicalise to: const $_0 = $_1 + $_2 ; return $_0 ;
        assert_eq!(hash_token_window(&a), hash_token_window(&b));
    }

    #[test]
    fn distinct_structure_hashes_differ() {
        let a = tokenise("const x = 1; const y = 2;");
        let b = tokenise("const x = 1; let y = 2;"); // const vs let
        assert_ne!(hash_token_window(&a), hash_token_window(&b));
    }

    #[test]
    fn tokenise_handles_unicode_identifiers() {
        // Greek letters + Han characters as identifier names. The
        // pre-cd-s2k ASCII-only scanner would treat each non-ASCII byte
        // as a single-byte operator token; with unicode-ident, they
        // form proper identifier tokens that get canonicalised.
        let toks = tokenise("const λ = 1; const 計算 = λ + 1;");
        let canons: Vec<&str> = toks.iter().map(|t| t.canon.as_str()).collect();
        assert_eq!(
            canons,
            vec!["const", "$_0", "=", "1", ";", "const", "$_1", "=", "$_0", "+", "1", ";"]
        );
        // Byte spans land on char boundaries, never mid-codepoint.
        for t in &toks {
            assert!(t.end as usize <= "const λ = 1; const 計算 = λ + 1;".len());
        }
    }

    #[test]
    fn rename_equivalent_unicode_and_ascii_match() {
        let ascii = tokenise("function add(x, y) { return x + y; }");
        let greek = tokenise("function add(α, β) { return α + β; }");
        assert_eq!(hash_token_window(&ascii), hash_token_window(&greek));
    }

    // ─── AST hash walker (cd-mti) tests ────────────────────────────────────
    //
    // Run a quick parse on a synthetic source, then compare hashes to verify
    // structural canonicalisation does what we expect.

    use cofferdam_core::parser::parse_into;
    use cofferdam_core::{Allocator, SourceFile};
    use std::path::PathBuf;

    fn ast_hash_first_n_stmts(text: &str, n: usize) -> u64 {
        let file = SourceFile::new(PathBuf::from("test.ts"), text.to_string());
        let alloc = Allocator::default();
        let parsed = parse_into(&alloc, &file);
        let mut walker = AstHashWalker::new();
        for stmt in parsed.program.body.iter().take(n) {
            walker.visit_statement(stmt);
        }
        walker.finish()
    }

    #[test]
    fn ast_hash_rename_equivalent_matches() {
        let a = "const total = price + tax; return total;";
        let b = "const sum = amount + fee; return sum;";
        assert_eq!(ast_hash_first_n_stmts(a, 2), ast_hash_first_n_stmts(b, 2));
    }

    #[test]
    fn ast_hash_brace_style_collapses() {
        // Block-with-braces vs single-statement consequent. The text
        // canonicaliser produced different hashes for these (because
        // `{` and `}` were tokens); the AST hasher should treat them
        // the same: an `if` with an ExpressionStatement consequent.
        //
        // (oxc actually wraps a single-statement consequent in a
        // BlockStatement only if braces are present in source, so the
        // two forms DO differ structurally — making this a useful
        // sanity check that the new hasher correctly distinguishes
        // them rather than hashes them equal.)
        let with_braces = "if (a) { b(); }";
        let without_braces = "if (a) b();";
        assert_ne!(
            ast_hash_first_n_stmts(with_braces, 1),
            ast_hash_first_n_stmts(without_braces, 1),
            "Block vs single-statement consequents are structurally different \
             AST shapes — they should hash distinctly"
        );
    }

    #[test]
    fn ast_hash_distinguishes_call_from_member() {
        // Same identifier, different shape: function call vs member access
        // SHOULD hash differently.
        let call = "foo(x);";
        let member = "foo.x;";
        assert_ne!(
            ast_hash_first_n_stmts(call, 1),
            ast_hash_first_n_stmts(member, 1)
        );
    }

    #[test]
    fn ast_hash_distinguishes_let_from_const() {
        let l = "let x = 1;";
        let c = "const x = 1;";
        assert_ne!(ast_hash_first_n_stmts(l, 1), ast_hash_first_n_stmts(c, 1));
    }

    #[test]
    fn ast_hash_distinguishes_literal_values() {
        let five = "if (x === 5) return;";
        let six = "if (x === 6) return;";
        assert_ne!(
            ast_hash_first_n_stmts(five, 1),
            ast_hash_first_n_stmts(six, 1)
        );
    }
}
