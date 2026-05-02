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
    ArrowFunctionExpression, BlockStatement, ConditionalExpression, DoWhileStatement,
    ForInStatement, ForOfStatement, ForStatement, Function, FunctionBody, IfStatement,
    LogicalExpression, Program, Statement, SwitchStatement, TryStatement, WhileStatement,
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
                severity: Severity::Warning,
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
/// Approximate v1: structural breaks (`if`, loops, `switch`, `catch`,
/// ternary) cost `1 + nesting`; logical operators (`&&` / `||` / `??`)
/// cost `1` flat. `else if` chains do not stack additional nesting.
/// Plain `else`, recursion, and Sonar's mixed-operator rule are
/// follow-ups; the goal at v1 is to surface the obvious deep-nest
/// offenders.
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
            nesting: 0,
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
    /// Nesting depth inside the current function. Reset on function entry
    /// (nested function bodies start fresh — Sonar treats them as new
    /// units).
    nesting: u32,
}

impl<'a> CogVisitor<'a> {
    fn enter(&mut self) {
        self.stack.push(0);
        self.nesting = 0;
    }

    fn exit(&mut self, name: String, span_start: u32, span_end: u32) {
        let count = self.stack.pop().unwrap_or(0);
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
                severity: Severity::Warning,
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
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        // Save & restore nesting so nested function bodies start fresh.
        let saved_nesting = self.nesting;
        self.enter();
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.exit(name, node.span.start, node.span.end);
        self.nesting = saved_nesting;
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        let saved_nesting = self.nesting;
        self.enter();
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.exit("arrow function".to_string(), node.span.start, node.span.end);
        self.nesting = saved_nesting;
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
        // sibling structural breaks. A plain `else { ... }` block walks
        // at +1 nesting.
        if let Some(alt) = &node.alternate {
            match alt {
                Statement::IfStatement(inner) => self.visit_if_statement(inner),
                other => {
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
        self.flat();
        oxc_ast_visit::walk::walk_logical_expression(self, node);
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

/// One canonicalised window of consecutive statements, recorded during
/// `Check::run`. Read back in `finalize` to find duplicates.
#[derive(Clone)]
struct Fingerprint {
    hash: u64,
    file: PathBuf,
    span: Span,
}

static DUPLICATE_BLOCKS: CorpusKey<Vec<Fingerprint>> =
    CorpusKey::new("Refactor.DuplicateBlock.fingerprints");

pub struct DuplicateBlock {
    min_statements: usize,
    min_chars: usize,
}

impl Default for DuplicateBlock {
    fn default() -> Self {
        Self {
            min_statements: DUPLICATE_BLOCK_MIN_STATEMENTS,
            min_chars: DUPLICATE_BLOCK_MIN_CHARS,
        }
    }
}

const DUP_META: CheckMeta = CheckMeta {
    id: "Refactor.DuplicateBlock",
    category: Category::Refactor,
    base_priority: 12,
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
        candidates.sort_by(|a, b| {
            a[0].file
                .cmp(&b[0].file)
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
            issues.push(Issue {
                check_id: DUP_META.id.to_string(),
                message: format!(
                    "duplicate {}-statement block, also at {} other location(s)",
                    self.min_statements,
                    related.len()
                ),
                file: primary.file.clone(),
                span: primary.span,
                priority: Priority(DUP_META.base_priority),
                severity: Severity::Warning,
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
            let slice = &self.file.text[start..end];
            if slice.len() < self.min_chars {
                continue;
            }
            let canon = canonicalise(slice);
            let hash = hash_str(&canon);
            let span = span_from_bytes(&self.file.text, start as u32, end as u32);
            self.collected.push(Fingerprint {
                hash,
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

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

/// Canonicalise a source-text slice for duplicate hashing.
///
/// - Identifier tokens map to `$_N` indices (first occurrence wins, so
///   two windows with all-different identifier names but identical
///   structure hash to the same value).
/// - JS/TS keywords stay verbatim (`let` ≠ `var`).
/// - Numeric/string literals stay verbatim (`x === 5` ≠ `x === 6`); a
///   future option could mask these for looser matching.
/// - Whitespace runs collapse to a single space; line + block comments
///   are stripped. Cosmetic noise should not differ-mask a duplicate.
///
/// ASCII identifier scan only at v1; non-ASCII identifier characters
/// fall through to the byte-copy branch and round-trip safely (they
/// just don't get index-substituted).
fn canonicalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut locals: HashMap<String, u32> = HashMap::new();
    let mut next: u32 = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if is_ident_start(b) {
            let start = i;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let word = &text[start..i];
            if is_keyword(word) {
                out.push_str(word);
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
                use std::fmt::Write;
                let _ = write!(out, "$_{}", idx);
            }
        } else if b.is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push(' ');
        } else if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            out.push(b as char);
            i += 1;
        }
    }
    out
}

fn hash_str(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
