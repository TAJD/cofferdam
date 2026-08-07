//! Warning checks — likely bugs. Default severity is `Error` for this
//! category once the per-category severity defaults wire up (phase 3).

use std::collections::HashSet;

use crate::framework_paths::is_framework_entry;
use cofferdam_core::public_api::{resolve_public_api, PublicApi};
use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    path_key, Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, Severity, SourceFile, TextEdit,
};
use oxc_ast::ast::{
    BinaryExpression, BinaryOperator, CallExpression, DebuggerStatement, Expression, NewExpression,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

/// `Warning.TripleEquals` — flags `==` and `!=` (vs `===` / `!==`).
///
/// Implementation note: pattern B from the SDK design (see
/// examplco_host_credo_checks memory). Walks every BinaryExpression
/// and checks the operator. The visitor is one-shot per file; phase-1
/// engine creates a fresh allocator + parsed view per file, runs all
/// checks, drops everything.
pub struct TripleEquals;

const META: CheckMeta = CheckMeta {
    id: "Warning.TripleEquals",
    category: Category::Warning,
    base_priority: 15,
    default_severity: Severity::High,
    explanation: "`==` and `!=` perform type coercion and are almost always a bug. Use `===` and `!==` instead.",
    body: include_str!("../docs/Warning.TripleEquals.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: true,
    pure_run: true,
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

    /// Replace the loose-equality operator with its strict equivalent.
    ///
    /// The issue span covers the whole `BinaryExpression` (e.g. `a == b`).
    /// We scan the source bytes within that span to locate just the operator
    /// token, taking care to skip `===`/`!==` and to handle `!=` before `==`
    /// so a `!=` sequence isn't misidentified as `=`.
    ///
    /// **Null exemption**: `== null` and `!= null` (either side) are intentional
    /// — `== null` matches both `null` and `undefined`, which `=== null` does
    /// not. Silently rewriting this changes semantics. We decline to fix those
    /// cases and return `None` so the diagnostic stays but no edit is applied.
    fn autofix(&self, issue: &Issue, source: &SourceFile) -> Option<TextEdit> {
        let start = issue.location.start_byte() as usize;
        let end = issue.location.end_byte() as usize;
        let slice = source.text.get(start..end)?;
        let bytes = slice.as_bytes();
        let len = bytes.len();

        // Scan for the operator token. We look for `!=` (not `!==`) and
        // `==` (not `===`). The approach: walk byte-by-byte and test each
        // position.
        let mut i = 0usize;
        while i < len {
            // Try `!=` at position i.
            if i + 1 < len && bytes[i] == b'!' && bytes[i + 1] == b'=' {
                // Make sure it's `!=` not `!==`.
                let is_strict = i + 2 < len && bytes[i + 2] == b'=';
                if !is_strict {
                    // Exempt `!= null` on either side — changing semantics.
                    if operand_is_null(bytes, 0, i) || operand_is_null(bytes, i + 2, len) {
                        return None;
                    }
                    let op_start = (start + i) as u32;
                    let op_end = op_start + 2;
                    let op_span = span_from_bytes(&source.text, op_start, op_end);
                    return Some(TextEdit {
                        span: op_span,
                        replacement: "!==".to_string(),
                    });
                }
                // Skip past `!==` entirely so we don't re-test the `=` bytes.
                i += 3;
                continue;
            }
            // Try `==` at position i (not `===`).
            if i + 1 < len && bytes[i] == b'=' && bytes[i + 1] == b'=' {
                let is_strict = i + 2 < len && bytes[i + 2] == b'=';
                if !is_strict {
                    // Exempt `== null` on either side — changing semantics.
                    if operand_is_null(bytes, 0, i) || operand_is_null(bytes, i + 2, len) {
                        return None;
                    }
                    let op_start = (start + i) as u32;
                    let op_end = op_start + 2;
                    let op_span = span_from_bytes(&source.text, op_start, op_end);
                    return Some(TextEdit {
                        span: op_span,
                        replacement: "===".to_string(),
                    });
                }
                // Skip past `===`.
                i += 3;
                continue;
            }
            i += 1;
        }
        None
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
            // Exempt `x == null`, `null == x`, `x != null`, `null != x`.
            // This is the ESLint "eqeqeq: always-except-null" semantics: the
            // `== null` pattern is idiomatic JS/TS shorthand for "is null OR
            // undefined", and replacing it with `=== null` changes behaviour.
            let either_side_is_null = matches!(node.left, Expression::NullLiteral(_))
                || matches!(node.right, Expression::NullLiteral(_));
            if !either_side_is_null {
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
                    location: Location::from_span(&self.file.path, span),
                    priority: Priority(META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                });
            }
        }
        // Walk into children — `==` can appear inside other binary ops
        // (e.g. `a && b == c`).
        oxc_ast_visit::walk::walk_binary_expression(self, node);
    }
}

/// `Warning.UnusedNullCheck` — the first type-aware built-in (cd-9hp.2).
///
/// Flags an equality comparison against `null` / `undefined` where the
/// other operand's resolved TypeScript type already excludes the value
/// being checked for — so the guard can never change the outcome. Either
/// dead defensive code, or a sign the annotation disagrees with reality.
///
/// Requires the ts-morph type host (`requires_types: true`): without an
/// installed `TypeOracle` the engine skips this check entirely. The
/// per-operand type query goes through `ctx.types` (see
/// `cofferdam-core::types` and `design/type-host-wire.md`).
///
/// Semantics mirror JS nullish equality precisely:
///
/// - `x == null` / `x != null` (loose) match BOTH null and undefined, so
///   they're redundant only when the type excludes both.
/// - `x === null` / `x !== null` (strict) check null alone.
/// - `x === undefined` / `x !== undefined` check undefined alone.
///
/// Bails on `any` / `unknown` — the compiler can't prove a guard
/// redundant against a type it knows nothing about.
pub struct UnusedNullCheck;

const UNUSED_NULL_CHECK_META: CheckMeta = CheckMeta {
    id: "Warning.UnusedNullCheck",
    category: Category::Warning,
    base_priority: 15,
    // Low by default: a redundant guard is a cleanup smell, not a
    // runtime bug (cofferdam trusts the types). Demote noise on real
    // repos; teams can raise it via cofferdam.toml.
    default_severity: Severity::Low,
    explanation: "An equality check against `null`/`undefined` whose other operand's TypeScript type already excludes that value — the guard can never change the outcome. Dead defensive code, or a hint the type annotation disagrees with reality.",
    body: include_str!("../docs/Warning.UnusedNullCheck.md"),
    requires_types: true,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for UnusedNullCheck {
    fn meta(&self) -> &'static CheckMeta {
        &UNUSED_NULL_CHECK_META
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
        let mut visitor = NullCheckCollector {
            file,
            types,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

#[derive(Clone, Copy)]
enum NullishKind {
    Null,
    Undefined,
}

/// Classify an operand as the `null` literal, the `undefined` identifier,
/// or neither. `undefined` is a global identifier in JS, not a literal —
/// a shadowed local `undefined` is rare enough to ignore in v1.
fn nullish_operand(expr: &Expression) -> Option<NullishKind> {
    match expr {
        Expression::NullLiteral(_) => Some(NullishKind::Null),
        Expression::Identifier(id) if id.name.as_str() == "undefined" => {
            Some(NullishKind::Undefined)
        }
        _ => None,
    }
}

struct NullCheckCollector<'a> {
    file: &'a SourceFile,
    types: &'a dyn cofferdam_core::TypeOracle,
    issues: Vec<Issue>,
}

impl<'a> NullCheckCollector<'a> {
    fn check_comparison(&mut self, node: &BinaryExpression<'a>) {
        let strict = match node.operator {
            BinaryOperator::Equality | BinaryOperator::Inequality => false,
            BinaryOperator::StrictEquality | BinaryOperator::StrictInequality => true,
            _ => return,
        };

        // Exactly one side must be a nullish literal; the other is the
        // value whose type we interrogate.
        let value = match (nullish_operand(&node.left), nullish_operand(&node.right)) {
            (Some(kind), None) => (kind, &node.right),
            (None, Some(kind)) => (kind, &node.left),
            // both nullish (`null == undefined`) or neither — not our pattern.
            _ => return,
        };
        let (kind, value_expr) = value;

        let span = value_expr.span();
        let Some(facts) = self.types.type_at(&self.file.path, span.start, span.end) else {
            return;
        };
        // Can't conclude anything against `any` / `unknown`.
        if facts.is_any {
            return;
        }

        // Redundant iff the type excludes everything the comparison tests
        // for. Loose ops (`==`/`!=`) test null OR undefined regardless of
        // which literal is written.
        let (redundant, excluded) = match (kind, strict) {
            (_, false) => (
                !facts.includes_null && !facts.includes_undefined,
                "null or undefined",
            ),
            (NullishKind::Null, true) => (!facts.includes_null, "null"),
            (NullishKind::Undefined, true) => (!facts.includes_undefined, "undefined"),
        };
        if !redundant {
            return;
        }

        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
        self.issues.push(Issue {
            check_id: UNUSED_NULL_CHECK_META.id.to_string(),
            message: format!(
                "redundant nullish check — value is typed `{}`, which already excludes {}",
                facts.text, excluded
            ),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(UNUSED_NULL_CHECK_META.base_priority),
            severity: Severity::Low,
            related: Vec::new(),
        });
    }
}

impl<'a> Visit<'a> for NullCheckCollector<'a> {
    fn visit_binary_expression(&mut self, node: &BinaryExpression<'a>) {
        self.check_comparison(node);
        // Nested comparisons (`a && b !== null`) live in children.
        oxc_ast_visit::walk::walk_binary_expression(self, node);
    }
}

/// `Warning.NoConsoleLog` — flags `console.log(...)` calls (and any
/// additional methods configured via the `methods` option).
///
/// Default scope is `console.log` only. Teams that want broader coverage
/// can expand it via `cofferdam.toml`:
///
/// ```toml
/// [checks."Warning.NoConsoleLog"]
/// methods = ["log", "warn", "error"]
/// ```
///
/// We only match the *bare* identifier `console`. Aliasing (`const c =
/// console; c.log(...)`) escapes detection — that's a known limit of
/// AST-only checks and not worth the complexity until the type-aware
/// pass lands.
pub struct NoConsoleLog;

const NO_CONSOLE_LOG_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "methods",
    kind: OptionKind::StringList,
    default: OptionDefault::StringList(&["log"]),
    doc: "console methods to flag. Defaults to [\"log\"]. Set to [\"log\", \"warn\", \"error\"] to restore the broad pre-v0.4 behaviour.",
}];

const NO_CONSOLE_LOG_META: CheckMeta = CheckMeta {
    id: "Warning.NoConsoleLog",
    category: Category::Warning,
    base_priority: -10,
    default_severity: Severity::Low,
    explanation: "`console.log(...)` calls are typically debugging leftovers. Route logs through a dedicated logger or strip them in CI.",
    body: include_str!("../docs/Warning.NoConsoleLog.md"),
    requires_types: false,
    consistency: false,
    options: NO_CONSOLE_LOG_OPTIONS,
    autofix: false,
    pure_run: true,
};

impl Check for NoConsoleLog {
    fn meta(&self) -> &'static CheckMeta {
        &NO_CONSOLE_LOG_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        // Collect the configured method set from options; fall back to the
        // schema default (`["log"]`) so unit tests that pass an empty
        // `CheckOptions` still behave predictably.
        let methods: Vec<String> = ctx
            .options
            .get_string_list("methods")
            .map(|xs| xs.to_vec())
            .unwrap_or_else(|| vec!["log".to_string()]);
        let mut visitor = ConsoleCollector {
            file,
            methods: &methods,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct ConsoleCollector<'a> {
    file: &'a SourceFile,
    /// Set of console method names to flag (e.g. `["log"]`).
    methods: &'a [String],
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for ConsoleCollector<'a> {
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &node.callee {
            if let Expression::Identifier(ident) = &member.object {
                if ident.name.as_str() == "console" {
                    let method = member.property.name.as_str();
                    if self.methods.iter().any(|m| m == method) {
                        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                        self.issues.push(Issue {
                            check_id: NO_CONSOLE_LOG_META.id.to_string(),
                            message: format!(
                                "remove `console.{}` call (use a logger or strip in CI)",
                                method
                            ),
                            file: self.file.path.clone(),
                            location: Location::from_span(&self.file.path, span),
                            priority: Priority(NO_CONSOLE_LOG_META.base_priority),
                            severity: Severity::Medium,
                            related: Vec::new(),
                        });
                    }
                }
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }
}

/// `Warning.NoDebugger` — flags every `debugger;` statement.
///
/// `debugger` halts execution under any attached devtools. Always a
/// debugging leftover in shipped code — there's no benign use case in
/// production builds. The fix is mechanical: delete the line.
pub struct NoDebugger;

const NO_DEBUGGER_META: CheckMeta = CheckMeta {
    id: "Warning.NoDebugger",
    category: Category::Warning,
    base_priority: 10,
    default_severity: Severity::Medium,
    explanation:
        "`debugger` statements halt execution under attached devtools. Remove before shipping.",
    body: include_str!("../docs/Warning.NoDebugger.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for NoDebugger {
    fn meta(&self) -> &'static CheckMeta {
        &NO_DEBUGGER_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = DebuggerCollector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct DebuggerCollector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for DebuggerCollector<'a> {
    fn visit_debugger_statement(&mut self, node: &DebuggerStatement) {
        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
        self.issues.push(Issue {
            check_id: NO_DEBUGGER_META.id.to_string(),
            message: "remove `debugger` statement".to_string(),
            file: self.file.path.clone(),
            location: Location::from_span(&self.file.path, span),
            priority: Priority(NO_DEBUGGER_META.base_priority),
            severity: Severity::Medium,
            related: Vec::new(),
        });
        // DebuggerStatement has no children, but call walk for symmetry
        // and so future oxc versions adding fields don't silently drop
        // them.
        oxc_ast_visit::walk::walk_debugger_statement(self, node);
    }
}

/// `Warning.NoEval` — flags `eval(...)` calls and `new Function(...)`.
///
/// Both forms execute arbitrary strings as code: `eval` directly,
/// `new Function(body)` via the Function constructor. Universally
/// banned in security-conscious codebases; this check has no opt-in
/// for a reason. Suppress per-line if you have a vetted, isolated use.
///
/// Aliasing (`const f = eval; f("...")`) is out of scope for the same
/// reason `NoConsoleLog` doesn't track aliases — bare-identifier match
/// is the line we draw without type info.
pub struct NoEval;

const NO_EVAL_META: CheckMeta = CheckMeta {
    id: "Warning.NoEval",
    category: Category::Warning,
    base_priority: 18,
    default_severity: Severity::High,
    explanation: "`eval(...)` and `new Function(...)` execute arbitrary strings as code. Universally banned for security and performance reasons.",
    body: include_str!("../docs/Warning.NoEval.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

impl Check for NoEval {
    fn meta(&self) -> &'static CheckMeta {
        &NO_EVAL_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = EvalCollector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct EvalCollector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for EvalCollector<'a> {
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::Identifier(ident) = &node.callee {
            if ident.name.as_str() == "eval" {
                let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                self.issues.push(Issue {
                    check_id: NO_EVAL_META.id.to_string(),
                    message: "avoid `eval(...)` — executes arbitrary strings as code".to_string(),
                    file: self.file.path.clone(),
                    location: Location::from_span(&self.file.path, span),
                    priority: Priority(NO_EVAL_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                });
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_new_expression(&mut self, node: &NewExpression<'a>) {
        if let Expression::Identifier(ident) = &node.callee {
            if ident.name.as_str() == "Function" {
                let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                self.issues.push(Issue {
                    check_id: NO_EVAL_META.id.to_string(),
                    message: "avoid `new Function(...)` — eval-equivalent code execution"
                        .to_string(),
                    file: self.file.path.clone(),
                    location: Location::from_span(&self.file.path, span),
                    priority: Priority(NO_EVAL_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                });
            }
        }
        oxc_ast_visit::walk::walk_new_expression(self, node);
    }
}

// ─── Warning.UnusedImport ──────────────────────────────────────────────────
//
// Project-aware unused-import: catches re-export passthroughs that
// single-file linters (tsc's noUnusedLocals, ESLint's no-unused-vars)
// miss because the file's own re-export "uses" the symbol from the
// file's perspective. We see the whole graph, so we can flag re-exports
// nobody downstream actually imports.
//
// Scope: re-export forwarders only. Plain unused imports inside a file
// are tsc's job.

const UI_META: CheckMeta = CheckMeta {
    id: "Warning.UnusedImport",
    category: Category::Warning,
    base_priority: 7,
    default_severity: Severity::Low,
    explanation: "Re-export of a symbol that no other file imports from this file. Single-file linters miss this case.",
    body: include_str!("../docs/Warning.UnusedImport.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

/// `Warning.UnusedImport` — finalize-stage check that flags imports
/// no consumer reads. Honours `[public_api]` exports (a re-export from
/// an allow-listed file is the published surface). See `CheckMeta`
/// for the full emission rules.
pub struct UnusedImport;

impl Check for UnusedImport {
    fn meta(&self) -> &'static CheckMeta {
        &UI_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        use cofferdam_core::graph::{
            ExportKind as GExportKind, ExportRecord as GExportRecord, ImportKind as GImportKind,
            ImportRecord as GImportRecord, InvariantsRuntime, EXPORTS as G_EXPORTS,
            IMPORTS as G_IMPORTS, INVARIANTS as G_INVARIANTS,
        };

        let imports: Vec<GImportRecord> = ctx
            .corpus
            .with_slot(&G_IMPORTS, |slot| slot.records().cloned().collect());
        let exports: Vec<GExportRecord> = ctx
            .corpus
            .with_slot(&G_EXPORTS, |slot| slot.records().cloned().collect());
        // [public_api] from cofferdam.invariants.toml. Re-exports from
        // files matched by `[public_api].exports` ARE the published
        // surface by construction (downstream consumers live in the
        // installed package and aren't visible in the corpus), so they
        // must not be flagged as orphan re-exports. Same exemption shape
        // as Design.OrphanExport (cd-5ej / cd-gro).
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&G_INVARIANTS, |slot| slot.clone());
        let public_api: PublicApi = runtime
            .as_ref()
            .map(|r| resolve_public_api(&r.public_api.exports, &r.project_root))
            .unwrap_or_default();

        // For each (re-exporter_path_key, name) — is anyone in the
        // project importing it?
        let mut named_consumed: HashSet<(String, String)> = HashSet::new();
        let mut default_consumed: HashSet<String> = HashSet::new();
        let mut ns_consumed: HashSet<String> = HashSet::new();
        for imp in &imports {
            let Some(resolved) = &imp.resolved else {
                continue;
            };
            let key = path_key(resolved);
            for n in &imp.names {
                match n.kind {
                    GImportKind::Default => {
                        default_consumed.insert(key.clone());
                    }
                    GImportKind::Namespace => {
                        ns_consumed.insert(key.clone());
                    }
                    GImportKind::Named => {
                        named_consumed.insert((key.clone(), n.source_name.clone()));
                    }
                }
            }
        }

        let mut issues = Vec::new();
        for exp in &exports {
            if !matches!(exp.kind, GExportKind::ReExport) {
                continue;
            }
            // Star re-exports (`export * from './m'`) are tracked as
            // `name = "*"` — namespace consumption is the matching shape.
            if exp.name == "*" {
                continue;
            }
            // Type-only re-exports: skip in v1, same false-positive
            // surface as DeadExport.
            if exp.type_only {
                continue;
            }
            // Framework-entry / config files: their re-exports are
            // consumed by the runtime, not by other user code. Pattern
            // list shared with `Design.OrphanExport` via
            // `crate::framework_paths` (cd-q53).
            if is_framework_entry(&exp.file) {
                continue;
            }
            let key = path_key(&exp.file);
            // Public API: re-exports from a `[public_api].exports`
            // file are the published surface; downstream consumers live
            // outside the corpus, so absence-of-import-in-project does
            // not mean unused (cd-5ej).
            if public_api.is_match(&key) {
                continue;
            }
            // If the re-exporter is itself reached by a namespace import
            // anywhere, every name on it is opaquely consumed.
            if ns_consumed.contains(&key) {
                continue;
            }
            let consumed = if exp.name == "default" {
                default_consumed.contains(&key)
            } else {
                named_consumed.contains(&(key.clone(), exp.name.clone()))
            };
            if consumed {
                continue;
            }
            let display = if exp.name == "default" {
                "default re-export".to_string()
            } else {
                format!("re-export `{}`", exp.name)
            };
            issues.push(Issue {
                check_id: UI_META.id.to_string(),
                message: format!("{} is never imported by another file", display),
                file: exp.file.clone(),
                location: Location::from_span(&exp.file, exp.span),
                priority: Priority(UI_META.base_priority),
                severity: UI_META.default_severity,
                related: Vec::new(),
            });
        }
        issues.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
        });
        issues
    }
}

/// Return `true` if the byte slice `bytes[from..to]`, after trimming ASCII
/// whitespace and recursively stripping balanced outer parentheses, is exactly
/// the token `null`.
///
/// This is intentionally a byte-level check, not a full re-parse. `null` is a
/// reserved TypeScript keyword so it cannot appear bare inside a string literal
/// or identifier without being the literal itself. The strip-parens step handles
/// `(foo || bar) == null` where the non-null operand has outer parens.
///
/// Word-boundary correctness: we compare the *entire* trimmed/stripped slice to
/// `b"null"`, so `nullable`, `"null"` (inside quotes), or `notNull` are not
/// matched.
/// Trim leading and trailing ASCII whitespace from a byte slice.
/// Equivalent to `slice.trim_ascii()` (stable since 1.80), but hand-rolled
/// so we stay within the workspace MSRV of 1.78.
fn trim_ascii_whitespace(slice: &[u8]) -> &[u8] {
    let start = slice
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(slice.len());
    let end = slice
        .iter()
        .rposition(|&b| !b.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(0);
    if start >= end {
        &[]
    } else {
        &slice[start..end]
    }
}

fn operand_is_null(bytes: &[u8], from: usize, to: usize) -> bool {
    let mut slice = match bytes.get(from..to) {
        Some(s) => s,
        None => return false,
    };
    // Trim leading and trailing ASCII whitespace.
    loop {
        slice = trim_ascii_whitespace(slice);
        // Strip one layer of balanced outer parens if present.
        if slice.first() == Some(&b'(') && slice.last() == Some(&b')') {
            // Verify the outer parens are actually a matched pair (not `(a) + (b)`).
            let mut depth = 0i32;
            let mut matched = false;
            for (idx, &b) in slice.iter().enumerate() {
                match b {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            matched = idx == slice.len() - 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if matched {
                slice = &slice[1..slice.len() - 1];
                continue; // re-trim after stripping
            }
        }
        break;
    }
    slice == b"null"
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{
        validate_options, Allocator, Check, CheckContext, CheckOptions, RawOptionValue, SourceFile,
    };
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Parse `src` as TypeScript, run `TripleEquals`, and return all issues.
    fn run_triple_equals(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        TripleEquals.run(&file, &mut ctx)
    }

    #[test]
    fn autofix_equality_returns_triple_equals() {
        // `a == b` — the operator is `==`, replacement should be `===`.
        let src = "const r = a == b;";
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1, "expected exactly one issue for `==`");
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should return Some for `==`");
        // The replacement text must be the strict form.
        assert_eq!(edit.replacement, "===");
        // The edit span must cover only the operator, not the whole expression.
        let op_slice = &src[edit.span.start_byte as usize..edit.span.end_byte as usize];
        assert_eq!(op_slice, "==");
    }

    #[test]
    fn autofix_inequality_returns_strict_not_equal() {
        // `a != b` — the operator is `!=`, replacement should be `!==`.
        let src = "const r = a != b;";
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1, "expected exactly one issue for `!=`");
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should return Some for `!=`");
        assert_eq!(edit.replacement, "!==");
        let op_slice = &src[edit.span.start_byte as usize..edit.span.end_byte as usize];
        assert_eq!(op_slice, "!=");
    }

    #[test]
    fn autofix_strict_equality_returns_none() {
        // `===` is already strict — no issue, so autofix is never called,
        // but verify the check produces zero issues.
        let src = "const r = a === b;";
        let issues = run_triple_equals(src);
        assert!(issues.is_empty(), "`===` should not be flagged");
    }

    #[test]
    fn autofix_strict_inequality_returns_none() {
        let src = "const r = a !== b;";
        let issues = run_triple_equals(src);
        assert!(issues.is_empty(), "`!==` should not be flagged");
    }

    // ── Null-exemption tests (cd-w9i) ─────────────────────────────────────
    //
    // ESLint "eqeqeq: always-except-null" semantics: the four null-operand
    // shapes are idiomatic JS/TS shorthand ("x is null OR undefined") and
    // must not fire at all — no diagnostic, no autofix.

    /// `x == null` — no diagnostic (null on RHS).
    #[test]
    fn null_rhs_eq_no_diagnostic() {
        let src = "const r = x == null;";
        let issues = run_triple_equals(src);
        assert!(
            issues.is_empty(),
            "`x == null` must not fire — idiom for null-or-undefined check"
        );
    }

    /// `x != null` — no diagnostic (null on RHS, inequality).
    #[test]
    fn null_rhs_neq_no_diagnostic() {
        let src = "const r = x != null;";
        let issues = run_triple_equals(src);
        assert!(
            issues.is_empty(),
            "`x != null` must not fire — idiom for null-or-undefined check"
        );
    }

    /// `null == x` — no diagnostic (null on LHS).
    #[test]
    fn null_lhs_eq_no_diagnostic() {
        let src = "const r = null == x;";
        let issues = run_triple_equals(src);
        assert!(
            issues.is_empty(),
            "`null == x` must not fire — null-operand exemption applies to LHS too"
        );
    }

    /// `null != x` — no diagnostic (null on LHS, inequality).
    #[test]
    fn null_lhs_neq_no_diagnostic() {
        let src = "const r = null != x;";
        let issues = run_triple_equals(src);
        assert!(
            issues.is_empty(),
            "`null != x` must not fire — null-operand exemption applies to LHS too"
        );
    }

    /// `(a || b) == null` — parenthesised non-null side; null on RHS still exempts.
    #[test]
    fn null_rhs_with_parens_no_diagnostic() {
        let src = "const r = (a || b) == null;";
        let issues = run_triple_equals(src);
        assert!(
            issues.is_empty(),
            "`(a || b) == null` must not fire — RHS is null literal"
        );
    }

    /// `a == b` — regression: non-null operands must still fire.
    #[test]
    fn non_null_operands_still_fire() {
        let src = "const r = a == b;";
        let issues = run_triple_equals(src);
        assert_eq!(
            issues.len(),
            1,
            "`a == b` must still fire — neither operand is null"
        );
    }

    /// `x != 0` — regression: non-null operands must still fire.
    #[test]
    fn non_null_numeric_still_fires() {
        let src = "const r = x != 0;";
        let issues = run_triple_equals(src);
        assert_eq!(
            issues.len(),
            1,
            "`x != 0` must still fire — 0 is not a null literal"
        );
    }

    /// `x == "null"` — string literal `"null"` is not the keyword; autofix proceeds.
    #[test]
    fn autofix_still_fixes_string_literal_null() {
        let src = r#"const r = x == "null";"#;
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1);
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should fire for string literal `\"null\"`");
        assert_eq!(edit.replacement, "===");
    }

    /// `nullable == y` — identifier that contains "null" is not the keyword; autofix proceeds.
    #[test]
    fn autofix_still_fixes_identifier_starting_with_null() {
        let src = "const r = nullable == y;";
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1);
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should fire for identifier `nullable`");
        assert_eq!(edit.replacement, "===");
    }

    // ── Warning.NoConsoleLog tests (cd-xin) ──────────────────────────────────

    /// Build a CheckOptions for NoConsoleLog with the given `methods` list.
    /// Pass `None` to use schema defaults (i.e. `["log"]`).
    fn console_opts(methods: Option<&[&str]>) -> CheckOptions {
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        if let Some(ms) = methods {
            raw.insert(
                "methods".to_string(),
                RawOptionValue::List(
                    ms.iter()
                        .map(|m| RawOptionValue::String(m.to_string()))
                        .collect(),
                ),
            );
        }
        validate_options(NO_CONSOLE_LOG_META.id, NO_CONSOLE_LOG_OPTIONS, &raw)
            .expect("valid options")
    }

    /// Parse `src` as TypeScript, run `NoConsoleLog` with the given options,
    /// and return all issues.
    fn run_no_console_log(src: &str, opts: &CheckOptions) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&parsed)
            .with_options(opts);
        NoConsoleLog.run(&file, &mut ctx)
    }

    /// Default config: `console.log` flagged.
    #[test]
    fn default_flags_console_log() {
        let opts = console_opts(None);
        let issues = run_no_console_log("console.log('debug');", &opts);
        assert_eq!(issues.len(), 1, "console.log must be flagged by default");
        assert!(issues[0].message.contains("console.log"));
    }

    /// Default config: `console.warn` NOT flagged.
    #[test]
    fn default_does_not_flag_console_warn() {
        let opts = console_opts(None);
        let issues = run_no_console_log("console.warn('warn');", &opts);
        assert!(
            issues.is_empty(),
            "console.warn must not be flagged by default"
        );
    }

    /// Default config: `console.error` NOT flagged.
    #[test]
    fn default_does_not_flag_console_error() {
        let opts = console_opts(None);
        let issues = run_no_console_log("console.error('err');", &opts);
        assert!(
            issues.is_empty(),
            "console.error must not be flagged by default"
        );
    }

    /// `methods = ["error"]`: only `console.error` flagged; `console.log` not.
    #[test]
    fn methods_error_only_flags_error() {
        let opts = console_opts(Some(&["error"]));
        let src = "console.log('a'); console.error('b');";
        let issues = run_no_console_log(src, &opts);
        assert_eq!(
            issues.len(),
            1,
            "only console.error should fire when methods=[\"error\"]"
        );
        assert!(issues[0].message.contains("console.error"));
    }

    /// `methods = ["log", "warn", "error"]`: all three flagged (old broad behaviour).
    #[test]
    fn methods_all_three_flags_all_three() {
        let opts = console_opts(Some(&["log", "warn", "error"]));
        let src = "console.log('a'); console.warn('b'); console.error('c');";
        let issues = run_no_console_log(src, &opts);
        assert_eq!(
            issues.len(),
            3,
            "log + warn + error must all fire when methods=[\"log\",\"warn\",\"error\"]"
        );
    }

    /// `methods = []`: nothing flagged.
    #[test]
    fn methods_empty_flags_nothing() {
        let opts = console_opts(Some(&[]));
        let src = "console.log('a'); console.warn('b'); console.error('c');";
        let issues = run_no_console_log(src, &opts);
        assert!(issues.is_empty(), "no issues should fire when methods=[]");
    }

    /// A method not in the list (`console.debug`) is not flagged by default.
    #[test]
    fn unlisted_method_not_flagged() {
        let opts = console_opts(None); // default: ["log"]
        let issues = run_no_console_log("console.debug('x');", &opts);
        assert!(
            issues.is_empty(),
            "console.debug must not fire when not in the methods list"
        );
    }

    /// Property-access without a call should not flag.
    #[test]
    fn member_without_call_not_flagged() {
        let opts = console_opts(None);
        // `console.log` as a reference (no call parens) — the visitor only
        // hits visit_call_expression, so non-call member access is safe.
        let issues = run_no_console_log("const fn = console.log;", &opts);
        assert!(
            issues.is_empty(),
            "bare member access (no call) must not fire"
        );
    }

    // --- Warning.UnusedNullCheck (cd-9hp.2.3) -------------------------
    //
    // The decision logic is type-driven, so a stub oracle stands in for
    // the ts-morph host: it returns the same TypeFacts for every query,
    // which is enough because each test source has one comparison. The
    // real worker path is covered by a gated test in cofferdam-cli.

    use cofferdam_core::{TypeFacts, TypeOracle};

    struct FixedOracle(TypeFacts);
    impl TypeOracle for FixedOracle {
        fn type_at(&self, _f: &Path, _s: u32, _e: u32) -> Option<TypeFacts> {
            Some(self.0.clone())
        }
    }

    fn facts(text: &str, null: bool, undef: bool, any: bool) -> TypeFacts {
        TypeFacts {
            text: text.into(),
            is_nullable: null || undef,
            includes_null: null,
            includes_undefined: undef,
            is_any: any,
        }
    }

    /// Parse `src`, run UnusedNullCheck with a stub oracle returning
    /// `f` for every type query.
    fn run_unused_null_check(src: &str, f: TypeFacts) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let oracle = FixedOracle(f);
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&parsed)
            .with_types(&oracle);
        UnusedNullCheck.run(&file, &mut ctx)
    }

    #[test]
    fn strict_null_check_on_non_null_type_is_flagged() {
        // `string` excludes null → `!== null` is redundant.
        let issues = run_unused_null_check(
            "function f(s: string) { if (s !== null) return s; }",
            facts("string", false, false, false),
        );
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("excludes null"));
    }

    #[test]
    fn strict_null_check_on_nullable_type_is_not_flagged() {
        // `string | null` includes null → the guard is meaningful.
        let issues = run_unused_null_check(
            "function f(s: string) { if (s !== null) return s; }",
            facts("string | null", true, false, false),
        );
        assert!(
            issues.is_empty(),
            "nullable type must not flag; got {issues:?}"
        );
    }

    #[test]
    fn loose_check_requires_excluding_both_null_and_undefined() {
        // `!= null` (loose) tests null OR undefined. A type that excludes
        // both → redundant.
        let flagged = run_unused_null_check(
            "function f(n: number) { if (n != null) return n; }",
            facts("number", false, false, false),
        );
        assert_eq!(flagged.len(), 1, "number excludes both → flag");

        // A type that still includes undefined → loose check is meaningful.
        let not_flagged = run_unused_null_check(
            "function f(n: number) { if (n != null) return n; }",
            facts("number | undefined", false, true, false),
        );
        assert!(
            not_flagged.is_empty(),
            "loose check stays meaningful when undefined is possible; got {not_flagged:?}"
        );
    }

    #[test]
    fn strict_undefined_check_keys_on_undefined_only() {
        // `=== undefined` against a type that excludes undefined but
        // INCLUDES null → still redundant (it can't be undefined).
        let issues = run_unused_null_check(
            "function f(s: string) { return s === undefined; }",
            facts("string | null", true, false, false),
        );
        assert_eq!(issues.len(), 1, "excludes undefined → flag; got {issues:?}");
    }

    #[test]
    fn any_type_is_never_flagged() {
        let issues = run_unused_null_check(
            "function f(x: any) { if (x != null) return x; }",
            facts("any", false, false, true),
        );
        assert!(issues.is_empty(), "any must bail; got {issues:?}");
    }

    #[test]
    fn non_nullish_comparison_is_ignored() {
        // Neither operand is null/undefined — the oracle is never even
        // consulted, so the (non-null) facts can't produce a finding.
        let issues = run_unused_null_check(
            "function f(a: number, b: number) { return a !== b; }",
            facts("number", false, false, false),
        );
        assert!(
            issues.is_empty(),
            "plain comparison must not flag; got {issues:?}"
        );
    }

    #[test]
    fn skipped_entirely_without_an_oracle() {
        // A requires_types check with no oracle installed emits nothing.
        let file = SourceFile::new(
            PathBuf::from("test.ts"),
            "function f(s: string) { if (s !== null) return s; }",
        );
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed); // no with_types
        let issues = UnusedNullCheck.run(&file, &mut ctx);
        assert!(issues.is_empty(), "no oracle → no findings; got {issues:?}");
    }
}
