use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, LineIndex,
    Location, OptionDefault, OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile,
    Span,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, BinaryExpression, BindingIdentifier,
    BlockStatement, BooleanLiteral, BreakStatement, CallExpression, Class, ConditionalExpression,
    ContinueStatement, DoWhileStatement, Expression, ExpressionStatement, ForInStatement,
    ForOfStatement, ForStatement, Function, FunctionBody, IdentifierName, IdentifierReference,
    IfStatement, LogicalExpression, NewExpression, NullLiteral, NumericLiteral, ReturnStatement,
    Statement, StringLiteral, SwitchStatement, TemplateLiteral, ThrowStatement, TryStatement,
    UnaryExpression, UpdateExpression, VariableDeclaration, WhileStatement,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

// ─── Refactor.NearDuplicateBlock ───────────────────────────────────────────
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
    /// Hash with string/number literal *values* included (never
    /// normalized to positional placeholders), regardless of the
    /// `normalize_literals` option. Used at finalize to tell whether a
    /// group of blocks sharing `hash` are truly identical or only
    /// identical in structure (CD-331).
    exact_hash: u64,
    kind: FingerprintKind,
    file: PathBuf,
    span: Span,
    /// Number of statements the window actually covered, read back at
    /// finalize so the message reports the effective (per-file
    /// configured) `min_statements` rather than the hardcoded
    /// `Default`. Unused in token mode — set to `0` there, since that
    /// message is built from `min_tokens` instead (mirrors how
    /// `exact_hash` is a no-op for token-mode grouping).
    stmt_count: usize,
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

/// `Refactor.NearDuplicateBlock` — flags repeated statement / token
/// sequences across the project corpus that differ only in literal
/// values. See `CheckMeta` for the emission contract and configurable
/// thresholds.
pub struct NearDuplicateBlock {
    min_statements: usize,
    min_chars: usize,
    /// Token-mode min window size. Only used when `include_tokens`.
    min_tokens: usize,
    /// Opt-in: also emit findings from a sliding token-window pass.
    /// Off by default — duplicates the work of AST mode for most
    /// hits, only paying off where copy-paste spans non-statement
    /// boundaries (a multi-line conditional broken across statements
    /// differently in two places, JSX runs, etc.). NOTE: token-mode
    /// fingerprints never differ in `exact_hash` vs `hash` (see
    /// `collect_token_fingerprints`), so they can only ever land in
    /// `partition_claimed_groups`'s exact-groups bucket — which this
    /// check does not emit. Token mode is currently inert here; it
    /// survives only because the old `Refactor.DuplicateBlock` (the
    /// exact-clone check, cut in CD-357 pass 2) was its sole consumer
    /// and this trim kept the collection machinery rather than
    /// deleting it outright.
    include_tokens: bool,
    /// AST-mode enabled (default: true). Can be disabled to run
    /// token-mode only. Configurable via cofferdam.toml.
    include_ast: bool,
    /// Treat string/number literals as positional placeholders (like
    /// identifiers) when computing the grouping hash, so blocks that
    /// differ only in their literal values are still reported as
    /// duplicates (CD-331). Default: true.
    normalize_literals: bool,
}

pub const DUP_BLOCK_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "min_statements",
        kind: OptionKind::Int,
        default: OptionDefault::Int(DUPLICATE_BLOCK_MIN_STATEMENTS as i64),
        doc: "minimum number of consecutive statements required to flag a duplicate block",
    },
    OptionSpec {
        name: "min_chars",
        kind: OptionKind::Int,
        default: OptionDefault::Int(DUPLICATE_BLOCK_MIN_CHARS as i64),
        doc: "minimum raw byte length of a duplicate window; prevents trivial single-liner runs from firing",
    },
    OptionSpec {
        name: "include_tokens",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(false),
        doc: "also run a sliding token-window pass that catches duplicates spanning non-statement boundaries",
    },
    OptionSpec {
        name: "include_ast",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(true),
        doc: "run the AST statement-window pass (disable to use token-mode only)",
    },
    OptionSpec {
        name: "normalize_literals",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(true),
        doc: "treat string and number literals as positional placeholders, so blocks differing only in their literal values are still reported as duplicates",
    },
];

impl Default for NearDuplicateBlock {
    fn default() -> Self {
        Self {
            min_statements: DUPLICATE_BLOCK_MIN_STATEMENTS,
            min_chars: DUPLICATE_BLOCK_MIN_CHARS,
            min_tokens: DUPLICATE_BLOCK_MIN_TOKENS,
            include_tokens: false,
            include_ast: true,
            normalize_literals: true,
        }
    }
}

impl NearDuplicateBlock {
    /// Construct with token-mode enabled and the given window size.
    /// AST mode stays on at default thresholds. See the `include_tokens`
    /// field doc: token-mode fingerprints never surface as findings on
    /// this check, so this constructor is currently only exercised for
    /// its (inert) effect on corpus collection.
    pub fn with_tokens(min_tokens: usize) -> Self {
        Self {
            include_tokens: true,
            min_tokens,
            ..Self::default()
        }
    }
}

/// `Refactor.NearDuplicateBlock` — the sole writer of `DUPLICATE_BLOCKS`
/// (CD-357 pass 2: absorbed `Refactor.DuplicateBlock`'s collection
/// logic when that check — the exact-clone half of the CD-331 split —
/// was cut as a linter-level check with no policy/graph angle). Reports
/// AST-mode groups whose members are structurally identical yet differ
/// in a string/number literal value; verbatim clones are collected into
/// the same corpus slot (for the self-overlap / cross-group dedup pass
/// in `partition_claimed_groups`) but are no longer surfaced by any
/// check — see `docs/Refactor.NearDuplicateBlock.md`.
const DUP_NEAR_META: CheckMeta = CheckMeta {
    id: "Refactor.NearDuplicateBlock",
    category: Category::Refactor,
    base_priority: 10,
    default_severity: Severity::Low,
    explanation: "Runs of statements that are structurally identical but differ in their string or number literals — often the same logic copied and then partially edited, where the edit is the thing worth looking at.",
    body: include_str!("../../docs/Refactor.NearDuplicateBlock.md"),
    requires_types: false,
    consistency: false,
    options: DUP_BLOCK_OPTIONS,
    autofix: false,
    // Writes per-file fingerprints into DUPLICATE_BLOCKS during run();
    // skipping run() on cache hit would drop those contributions, so
    // we always re-run this one. cp3's corpus-snapshot replay will
    // lift this restriction.
    pure_run: false,
};

impl Check for NearDuplicateBlock {
    fn meta(&self) -> &'static CheckMeta {
        &DUP_NEAR_META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&DUPLICATE_BLOCKS, |slot, path| {
            slot.retain(|f| f.file != path)
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let min_statements = ctx
            .options
            .get_int("min_statements")
            .map(|v| v as usize)
            .unwrap_or(self.min_statements);
        let min_chars = ctx
            .options
            .get_int("min_chars")
            .map(|v| v as usize)
            .unwrap_or(self.min_chars);
        // Bool flags: merge constructor opt-in with TOML config.
        // `self.include_tokens` is true when the check was constructed via
        // `with_tokens()`; TOML sets it to true for the default-constructed
        // instance. Either source suffices — enabling is additive.
        let include_tokens =
            self.include_tokens || ctx.options.get_bool("include_tokens").unwrap_or(false);
        // `include_ast` defaults to true. TOML can turn it off; the struct
        // field can also turn it off (no existing constructor does this, but
        // the option is there for completeness).
        let include_ast = self.include_ast && ctx.options.get_bool("include_ast").unwrap_or(true);
        let normalize_literals = ctx
            .options
            .get_bool("normalize_literals")
            .unwrap_or(self.normalize_literals);

        let mut collected = Vec::new();
        // Built once per file: `span_from_bytes` is O(start_byte), and a
        // file can produce dozens of candidate windows, nearly all of
        // which are discarded before ever becoming a finding (CD-140-perf
        // audit finding #1 — this was the single largest per-check cost
        // in a real-corpus profile). A shared line-start table turns each
        // resolution into O(log n) instead.
        let line_index = LineIndex::new(&file.text);

        if include_ast {
            let mut visitor = DupCollector {
                file,
                line_index: &line_index,
                min_statements,
                min_chars,
                normalize_literals,
                collected: Vec::new(),
            };
            visitor.visit_program(parsed.program);
            collected.append(&mut visitor.collected);
        }

        if include_tokens {
            collect_token_fingerprints(
                file,
                &line_index,
                self.min_tokens,
                min_chars,
                &mut collected,
            );
        }

        ctx.corpus.with_slot(&DUPLICATE_BLOCKS, |slot| {
            slot.append(&mut collected);
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        // Only the literal-drift half belongs to this check id (CD-331
        // follow-up split); the exact half (verbatim clones, plus all
        // token-mode groups — token mode's exact_hash always equals
        // hash, see `collect_token_fingerprints`) is collected into the
        // same corpus slot for the dedup pass but is no longer surfaced
        // by any check, since `Refactor.DuplicateBlock` was cut in
        // CD-357 pass 2.
        let (_exact_groups, near_groups) = partition_claimed_groups(ctx);
        near_groups
            .into_iter()
            .map(|group| {
                let message = format!(
                    "duplicate {}-statement block differing only in literal values, also at {} other location(s)",
                    group[0].stmt_count,
                    group.len() - 1
                );
                build_issue(DUP_NEAR_META.id, DUP_NEAR_META.base_priority, message, &group)
            })
            .collect()
    }
}

/// Test-only: emits an `Issue` for every claimed group (exact clones
/// *and* literal-drift near-duplicates), not just the near half that
/// `NearDuplicateBlock::finalize` surfaces in production. The windowing
/// / dedup / exclusion logic under `partition_claimed_groups` is shared
/// by both halves regardless of which check (if any) reports a given
/// group, so the CD-147 / CD-331 / CD-339 regression tests in
/// `refactor::tests` use this to keep exercising that shared logic via
/// verbatim-clone fixtures, even though `Refactor.DuplicateBlock` (the
/// check that used to report the exact half) was cut in CD-357 pass 2.
#[cfg(test)]
pub(crate) fn finalize_all_groups_for_test(ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
    let (exact_groups, near_groups) = partition_claimed_groups(ctx);
    exact_groups
        .into_iter()
        .chain(near_groups)
        .map(|group| {
            let message = format!(
                "duplicate {}-statement block, also at {} other location(s)",
                group[0].stmt_count,
                group.len() - 1
            );
            build_issue(
                DUP_NEAR_META.id,
                DUP_NEAR_META.base_priority,
                message,
                &group,
            )
        })
        .collect()
}

/// `Issue` construction for `NearDuplicateBlock` findings.
fn build_issue(
    check_id: &'static str,
    base_priority: i8,
    message: String,
    group: &[Fingerprint],
) -> Issue {
    let primary = &group[0];
    let related: Vec<RelatedSpan> = group[1..]
        .iter()
        .map(|fp| RelatedSpan {
            location: Location::from_span(&fp.file, fp.span),
            file: fp.file.clone(),
        })
        .collect();
    Issue {
        check_id: check_id.to_string(),
        message,
        file: primary.file.clone(),
        location: Location::from_span(&primary.file, primary.span),
        priority: Priority(base_priority),
        severity: Severity::Medium,
        related,
    }
}

/// Shared finalize core for `DuplicateBlock` and `NearDuplicateBlock`:
/// reads `DUPLICATE_BLOCKS`, groups by `hash`, dedupes self-overlaps,
/// and runs the cross-group overlap-claim pass exactly once — then
/// partitions the *emitted* groups into "exact" (every member shares
/// the primary's `exact_hash` — verbatim clones, and all token-mode
/// groups) and "near" (an AST-mode group whose members differ in a
/// literal value).
///
/// Splitting after the claim pass, not before, is what keeps the two
/// checks from ever reporting overlapping spans: the claim pass is
/// identical to (and, called once per check, reproduces) the single
/// pass this file ran before the CD-331 split, so `exact_groups ∪
/// near_groups` is exactly the set of groups the one check used to
/// emit, and the two checks between them cover it once each rather
/// than only recomputing the same deterministic partition twice.
fn partition_claimed_groups(
    ctx: &mut FinalizeContext<'_>,
) -> (Vec<Vec<Fingerprint>>, Vec<Vec<Fingerprint>>) {
    let mut by_hash: BTreeMap<u64, Vec<Fingerprint>> = BTreeMap::new();
    // Read-only (cd-32): a draining read would empty the slot as a
    // side effect of finalize, which is fine for a one-shot analyze
    // but corrupts `Engine::analyze_incremental`'s persistent
    // `AnalysisState` — the next incremental call would finalize
    // over an empty slot for every file that didn't just change.
    ctx.corpus.with_slot(&DUPLICATE_BLOCKS, |slot| {
        for fp in slot.iter().cloned() {
            by_hash.entry(fp.hash).or_default().push(fp);
        }
    });

    // Build candidate findings and dedupe overlapping windows in the
    // same file. Sort groups by their primary's (file, start_byte) so
    // earlier-in-the-source windows claim the territory first.
    let mut candidates: Vec<Vec<Fingerprint>> = by_hash
        .into_values()
        .map(|mut fps| {
            fps.sort_by(|a, b| {
                a.file
                    .cmp(&b.file)
                    .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
            });
            dedupe_self_overlaps(fps)
        })
        // Self-overlap dedup can shrink a group below 2 (a single
        // real block whose sliding windows all hashed identically),
        // so re-check the size floor after it, not before.
        .filter(|fps| fps.len() >= 2)
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
    let mut exact_groups = Vec::new();
    let mut near_groups = Vec::new();

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

        // Grouping happens on `hash` (normalized under
        // `normalize_literals`); check whether the group is also
        // identical in its exact (literal-sensitive) hash to decide
        // which check id owns it (CD-331 / CD-331 follow-up).
        let all_exact = group.iter().all(|fp| fp.exact_hash == primary.exact_hash);
        if all_exact {
            exact_groups.push(group);
        } else {
            near_groups.push(group);
        }
    }
    (exact_groups, near_groups)
}

/// Collapse windows *within one hash group* that overlap each other in
/// the same file. `fps` is pre-sorted by `(file, start_byte)`, so a
/// contiguous run of mutually-overlapping windows is walked in order
/// and only the first (earliest-starting) window per run is kept.
///
/// Needed because a uniform field-copy chain (e.g. six back-to-back
/// `this.x = value.x;` lines) canonicalizes every sliding window to
/// the *same* structural hash — local-identifier numbering restarts at
/// 0 within each window, so a window starting one statement later
/// hashes identically to the one before it. Without this pass, those
/// self-overlapping windows land in one hash group and get reported as
/// spurious "duplicates" of themselves (CD-147); the cross-group
/// `claimed` check in the caller only guards against a *different*
/// hash group re-covering already-emitted territory, not against
/// overlap inside the same group.
fn dedupe_self_overlaps(fps: Vec<Fingerprint>) -> Vec<Fingerprint> {
    let mut kept: Vec<Fingerprint> = Vec::with_capacity(fps.len());
    for fp in fps {
        let overlaps_prev = kept.last().is_some_and(|prev: &Fingerprint| {
            prev.file == fp.file
                && prev.span.start_byte < fp.span.end_byte
                && fp.span.start_byte < prev.span.end_byte
        });
        if !overlaps_prev {
            kept.push(fp);
        }
    }
    kept
}

struct DupCollector<'a> {
    file: &'a SourceFile,
    line_index: &'a LineIndex,
    min_statements: usize,
    min_chars: usize,
    normalize_literals: bool,
    collected: Vec<Fingerprint>,
}

impl<'a> DupCollector<'a> {
    fn scan(&mut self, stmts: &[Statement<'a>]) {
        // Module-level import/re-export declarations are excluded from
        // windowing entirely (CD-331 follow-up), not just as window
        // starts — otherwise a window could straddle the import block
        // into real code. A run of import statements differing only in
        // their module specifiers is never actionable (you cannot
        // extract a shared helper for an import block), and once
        // literal normalization treats those specifiers as positional
        // placeholders, every same-length import block in a project
        // hashes identically. Plain re-exports (`export { x } from
        // './y'`) are excluded the same way; a bare `export { x }` or
        // `export function f() {}` (no `source`) is ordinary code and
        // still participates.
        //
        // Windowing runs per maximal contiguous run of non-excluded
        // statements (CD-339) rather than over a flattened list of all
        // of them — a flattened list lets a window's `start`/`end` span
        // an excluded statement sitting between two real ones, so the
        // reported range (and the `min_chars` floor computed from it)
        // included bytes the window never actually hashed.
        let mut run: Vec<&Statement<'a>> = Vec::new();
        for stmt in stmts {
            if is_import_or_reexport(stmt) {
                self.scan_run(std::mem::take(&mut run));
            } else {
                run.push(stmt);
            }
        }
        self.scan_run(run);
    }

    fn scan_run(&mut self, stmts: Vec<&Statement<'a>>) {
        if stmts.len() < self.min_statements {
            return;
        }
        // Per-statement hash-op streams (CD-173), memoized lazily: a
        // statement is AST-walked at most once per run no matter how
        // many overlapping windows it participates in (previously it
        // was walked once per window, i.e. up to `min_statements`
        // times). Lazy rather than eager so scopes where every window
        // gets filtered out by `min_chars` below don't pay for op
        // collection they'll never use.
        let mut stmt_ops: Vec<Option<Vec<HashOp<'a>>>> = (0..stmts.len()).map(|_| None).collect();
        for i in 0..=stmts.len() - self.min_statements {
            let first = stmts[i];
            let last = stmts[i + self.min_statements - 1];
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
            for j in i..i + self.min_statements {
                if stmt_ops[j].is_none() {
                    stmt_ops[j] = Some(collect_stmt_ops(stmts[j]));
                }
            }
            let window = &stmt_ops[i..i + self.min_statements];
            let (normalized, exact) = hash_ops(
                window
                    .iter()
                    .flat_map(|ops| ops.as_ref().expect("just populated above").iter()),
            );
            // When normalization is off, the grouping hash IS the exact
            // hash — that reproduces pre-CD-331 grouping behaviour and
            // keeps the "is this group exact?" check trivially true.
            let hash = if self.normalize_literals {
                normalized
            } else {
                exact
            };
            let span = self.line_index.span_from_bytes(start as u32, end as u32);
            self.collected.push(Fingerprint {
                hash,
                exact_hash: exact,
                kind: FingerprintKind::Ast,
                file: self.file.path.clone(),
                span,
                stmt_count: self.min_statements,
            });
        }
    }
}

/// True for statements that must never enter an AST-mode window
/// (CD-331 follow-up, extended CD-339): plain `import` declarations,
/// `export * from`, and re-exports (`export { x } from './y'`); the
/// TS-specific `import x = require('y')` (`TSImportEqualsDeclaration`)
/// and `export = x` (`TSExportAssignment`) spellings; and a
/// `VariableDeclaration` that is a CommonJS require block — every
/// declarator's initialiser is a direct `require(...)` call (a
/// declarator with no initialiser, or one whose initialiser is
/// anything else, makes the whole statement ordinary code instead). A
/// bare `export { x }` or `export function f() {}` — an
/// `ExportNamedDeclaration` with no `source` — is ordinary code and is
/// NOT excluded.
fn is_import_or_reexport(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::ImportDeclaration(_) => true,
        Statement::ExportAllDeclaration(_) => true,
        Statement::ExportNamedDeclaration(decl) => decl.source.is_some(),
        Statement::TSImportEqualsDeclaration(_) => true,
        Statement::TSExportAssignment(_) => true,
        Statement::VariableDeclaration(decl) => {
            !decl.declarations.is_empty()
                && decl
                    .declarations
                    .iter()
                    .all(|d| d.init.as_ref().is_some_and(is_require_call))
        }
        _ => false,
    }
}

/// True for `require(...)` — a `CallExpression` whose callee is the bare
/// identifier `require`. Used to recognise `const x = require('y')` (and
/// destructured variants like `const { a } = require('y')`) as an import
/// for windowing purposes.
fn is_require_call(expr: &Expression<'_>) -> bool {
    let Expression::CallExpression(call) = expr else {
        return false;
    };
    matches!(&call.callee, Expression::Identifier(id) if id.name == "require")
}

impl<'a> Visit<'a> for DupCollector<'a> {
    fn visit_program(&mut self, node: &oxc_ast::ast::Program<'a>) {
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

/// Delimits fields within a single hashed node so that concatenated tag
/// bytes can't collide across a boundary (e.g. `"ab"+"c"` hashing the
/// same as `"a"+"bc"`). Written after every tag/value via `tag()`,
/// `ident_tag()`, and the string/numeric literal visitors below.
///
/// Footgun: changing this value changes every `hash_ast_window` output,
/// which silently invalidates every previously-cached/compared
/// `DuplicateBlock` fingerprint. Do not change casually.
const HASH_SEPARATOR: u8 = 0x01;

/// Tag → AST node type registry. Every tag below is written via
/// `hasher.write` / `write_all` (directly or through `tag()`/`ident_tag()`)
/// before a node's contents are hashed, so a block and an
/// expression-statement with byte-identical contents still hash
/// differently. Adding, removing, or renaming ANY tag changes every
/// existing `DuplicateBlock` hash — treat this table as append-only in
/// practice (don't recycle a retired tag for a different node type).
///
/// | Tag      | AST node type                                    |
/// |----------|---------------------------------------------------|
/// | `Blk`    | `BlockStatement`                                   |
/// | `ExpS`   | `ExpressionStatement`                               |
/// | `If`     | `IfStatement`                                       |
/// | `+E`     | `IfStatement` with an `alternate` (else branch)     |
/// | `For`    | `ForStatement`                                      |
/// | `ForIn`  | `ForInStatement`                                    |
/// | `ForOf`  | `ForOfStatement`                                    |
/// | `Whl`    | `WhileStatement`                                    |
/// | `Do`     | `DoWhileStatement`                                  |
/// | `Sw`     | `SwitchStatement`                                   |
/// | `Try`    | `TryStatement`                                      |
/// | `+C`     | `TryStatement` with a `handler` (catch clause)      |
/// | `+F`     | `TryStatement` with a `finalizer` (finally block)   |
/// | `Ret`    | `ReturnStatement`                                   |
/// | `Thr`    | `ThrowStatement`                                    |
/// | `Brk`    | `BreakStatement`                                    |
/// | `Cnt`    | `ContinueStatement`                                 |
/// | `Var:`   | `VariableDeclaration` (followed by `kind`, e.g. `let`/`const`/`var`) |
/// | `Fn`     | `Function`                                          |
/// | `Arrow`  | `ArrowFunctionExpression`                           |
/// | `Cls`    | `Class`                                             |
/// | `Bin:`   | `BinaryExpression` (followed by the operator string) |
/// | `Log:`   | `LogicalExpression` (followed by the operator string) |
/// | `Una:`   | `UnaryExpression` (followed by the operator string) |
/// | `Upd:`   | `UpdateExpression` (followed by the operator string, then `P`/`S` for prefix/postfix) |
/// | `Asn:`   | `AssignmentExpression` (followed by the operator string) |
/// | `Tern`   | `ConditionalExpression`                             |
/// | `Call`   | `CallExpression`                                    |
/// | `New`    | `NewExpression`                                     |
/// | `Id:`    | `IdentifierReference` (followed by its local index) |
/// | `Bid:`   | `BindingIdentifier` (followed by its local index)   |
/// | `Idn:`   | `IdentifierName` (followed by its local index)      |
/// | `Str:`   | `StringLiteral` (followed by the literal's value)   |
/// | `Num:`   | `NumericLiteral` (followed by the literal's little-endian f64 bytes) |
/// | `T`      | `BooleanLiteral` with value `true`                  |
/// | `F`      | `BooleanLiteral` with value `false`                 |
/// | `Nul`    | `NullLiteral`                                       |
/// | `Tmpl`   | `TemplateLiteral`                                   |
/// One structural emission from walking a statement's AST, deferred
/// rather than fed straight into a hasher (CD-173). Every variant
/// borrows straight from the oxc arena (`'a`, the parsed file's
/// lifetime) or is `Copy` data — zero heap allocation per op, since an
/// early version of this that owned (`Box<str>`/`Vec<u8>`) each op
/// turned out to allocate MORE than the original per-window AST walk
/// it replaced (measured: a real regression on `bestefforttools`, not
/// an improvement) once every identifier occurrence — not just the
/// first per scope — got its own heap allocation.
/// `Ident` carries an identifier occurrence whose local index can only
/// be resolved once the full window it belongs to is known (see
/// `hash_ops`) — the same name may need a different index depending on
/// what window-relative position it's first seen at.
#[derive(Clone, Copy)]
enum HashOp<'a> {
    /// A plain structural tag. Every call site passes a literal byte
    /// string or an oxc `.as_str()` result — both confirmed `&'static
    /// str` in oxc_syntax/oxc_ast (operator/kind enums render via a
    /// `match` over literals, never borrowing from `self`).
    Bytes(&'static [u8]),
    Ident {
        prefix: &'static [u8],
        name: &'a str,
    },
    /// `StringLiteral` value, borrowed from the arena.
    Str(&'a str),
    /// `NumericLiteral` value.
    Num(f64),
}

/// Combine a window's worth of precomputed `HashOp`s into the final
/// structural hash, assigning each distinct identifier name the next
/// available index in first-occurrence order *within this window* —
/// this window-relative (not per-statement, not per-file) numbering is
/// exactly what `AstHashWalker`'s old single-pass `locals`/`next_local`
/// fields did when shared across one window's worth of
/// `visit_statement` calls; splitting collection from combination
/// preserves it because op emission order for a given statement never
/// depends on sibling statements' content (see `visit_*` below — every
/// branch is on the node itself, never on accumulated hasher/locals
/// state), so concatenating precomputed per-statement op lists in
/// statement order reproduces the original single-pass sequence
/// exactly.
/// Combines a window's `HashOp`s into two hashes in a single pass:
/// `(normalized, exact)`. `normalized` treats string/number literals as
/// positional placeholders (same idea as identifier canonicalisation);
/// `exact` hashes literal values verbatim, as `hash_ops` always did
/// before CD-331. Computed together — not via two calls — because this
/// is a hot path (CD-173).
fn hash_ops<'a, 'i>(ops: impl Iterator<Item = &'i HashOp<'a>>) -> (u64, u64)
where
    'a: 'i,
{
    use std::hash::Hasher;
    let mut norm_hasher = std::collections::hash_map::DefaultHasher::new();
    let mut exact_hasher = std::collections::hash_map::DefaultHasher::new();
    let mut locals: HashMap<&str, u32> = HashMap::new();
    let mut next_local: u32 = 0;
    let mut str_locals: HashMap<&str, u32> = HashMap::new();
    let mut next_str: u32 = 0;
    let mut num_locals: HashMap<u64, u32> = HashMap::new();
    let mut next_num: u32 = 0;
    for op in ops {
        match *op {
            HashOp::Bytes(bytes) => {
                norm_hasher.write(bytes);
                norm_hasher.write_u8(HASH_SEPARATOR);
                exact_hasher.write(bytes);
                exact_hasher.write_u8(HASH_SEPARATOR);
            }
            HashOp::Ident { prefix, name } => {
                let idx = match locals.get(name) {
                    Some(&i) => i,
                    None => {
                        let i = next_local;
                        next_local += 1;
                        locals.insert(name, i);
                        i
                    }
                };
                norm_hasher.write(prefix);
                norm_hasher.write_u32(idx);
                norm_hasher.write_u8(HASH_SEPARATOR);
                exact_hasher.write(prefix);
                exact_hasher.write_u32(idx);
                exact_hasher.write_u8(HASH_SEPARATOR);
            }
            HashOp::Str(value) => {
                let idx = match str_locals.get(value) {
                    Some(&i) => i,
                    None => {
                        let i = next_str;
                        next_str += 1;
                        str_locals.insert(value, i);
                        i
                    }
                };
                norm_hasher.write(b"Str#");
                norm_hasher.write_u32(idx);
                norm_hasher.write_u8(HASH_SEPARATOR);

                exact_hasher.write(b"Str:");
                exact_hasher.write(value.as_bytes());
                exact_hasher.write_u8(HASH_SEPARATOR);
            }
            HashOp::Num(value) => {
                let bits = value.to_bits();
                let idx = match num_locals.get(&bits) {
                    Some(&i) => i,
                    None => {
                        let i = next_num;
                        next_num += 1;
                        num_locals.insert(bits, i);
                        i
                    }
                };
                norm_hasher.write(b"Num#");
                norm_hasher.write_u32(idx);
                norm_hasher.write_u8(HASH_SEPARATOR);

                exact_hasher.write(b"Num:");
                exact_hasher.write(&value.to_le_bytes());
                exact_hasher.write_u8(HASH_SEPARATOR);
            }
        }
    }
    (norm_hasher.finish(), exact_hasher.finish())
}

pub struct AstHashWalker<'a> {
    ops: Vec<HashOp<'a>>,
}

impl<'a> AstHashWalker<'a> {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    fn tag(&mut self, bytes: &'static [u8]) {
        self.ops.push(HashOp::Bytes(bytes));
    }

    fn ident_tag(&mut self, prefix: &'static [u8], name: &'a str) {
        self.ops.push(HashOp::Ident { prefix, name });
    }

    #[allow(dead_code)] // exercised by refactor::tests' AST-hash canonicalisation sanity checks
    pub fn finish(self) -> (u64, u64) {
        hash_ops(self.ops.iter())
    }
}

impl<'a> Visit<'a> for AstHashWalker<'a> {
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
        self.ops.push(HashOp::Str(lit.value.as_str()));
    }

    fn visit_numeric_literal(&mut self, lit: &NumericLiteral<'a>) {
        self.ops.push(HashOp::Num(lit.value));
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

/// Walk one statement's AST into its raw (unresolved-identifier) op
/// stream. Called at most once per statement per `scan()` (CD-173) —
/// callers combine a window's worth of these via `hash_ops` rather
/// than re-walking the AST for every overlapping window a statement
/// participates in. Token mode still uses text canonicalisation.
fn collect_stmt_ops<'a>(stmt: &Statement<'a>) -> Vec<HashOp<'a>> {
    let mut walker = AstHashWalker::new();
    walker.visit_statement(stmt);
    walker.ops
}

// ─── Token mode (cd-jdq) ───────────────────────────────────────────────────

/// One canonicalised token plus its byte span in the original source.
/// Token mode slides a window over a Vec of these.
#[derive(Clone)]
pub struct TokenInfo {
    pub canon: String,
    pub start: u32,
    pub end: u32,
}

/// Tokenise the file's source into canonicalised tokens with spans.
/// (Exported for testing.)
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
pub fn tokenise(text: &str) -> Vec<TokenInfo> {
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

pub fn hash_token_window(window: &[TokenInfo]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let mut h = DefaultHasher::new();
    for t in window {
        h.write(t.canon.as_bytes());
        // Separator so adjacent tokens can't collide with a longer one
        // (e.g. `[` `]` should not hash like `[]`).
        h.write_u8(HASH_SEPARATOR);
    }
    h.finish()
}

fn collect_token_fingerprints(
    file: &SourceFile,
    line_index: &LineIndex,
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
        let span = line_index.span_from_bytes(start, end);
        out.push(Fingerprint {
            hash,
            // Token mode doesn't go through `hash_ops` and is out of
            // scope for CD-331's literal-normalization; exact == hash
            // here so the finalize "is this group exact?" check stays
            // trivially true for token-mode findings.
            exact_hash: hash,
            kind: FingerprintKind::Token,
            file: file.path.clone(),
            span,
            // Unused by the token-mode message (see `Fingerprint::stmt_count`'s doc).
            stmt_count: 0,
        });
    }
}
