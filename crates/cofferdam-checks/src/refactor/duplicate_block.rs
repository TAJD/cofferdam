use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, AssignmentExpression, BinaryExpression, BindingIdentifier,
    BlockStatement, BooleanLiteral, BreakStatement, CallExpression, Class, ConditionalExpression,
    ContinueStatement, DoWhileStatement, ExpressionStatement, ForInStatement, ForOfStatement,
    ForStatement, Function, FunctionBody, IdentifierName, IdentifierReference, IfStatement,
    LogicalExpression, NewExpression, NullLiteral, NumericLiteral, ReturnStatement, Statement,
    StringLiteral, SwitchStatement, TemplateLiteral, ThrowStatement, TryStatement, UnaryExpression,
    UpdateExpression, VariableDeclaration, WhileStatement,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

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

/// `Refactor.DuplicateBlock` — flags repeated statement / token
/// sequences across the project corpus. See `CheckMeta` for the
/// emission contract and configurable thresholds.
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
    /// AST-mode enabled (default: true). Can be disabled to run
    /// token-mode only. Configurable via cofferdam.toml.
    include_ast: bool,
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
];

impl Default for DuplicateBlock {
    fn default() -> Self {
        Self {
            min_statements: DUPLICATE_BLOCK_MIN_STATEMENTS,
            min_chars: DUPLICATE_BLOCK_MIN_CHARS,
            min_tokens: DUPLICATE_BLOCK_MIN_TOKENS,
            include_tokens: false,
            include_ast: true,
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
    body: include_str!("../../docs/Refactor.DuplicateBlock.md"),
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

impl Check for DuplicateBlock {
    fn meta(&self) -> &'static CheckMeta {
        &DUP_META
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

        let mut collected = Vec::new();

        if include_ast {
            let mut visitor = DupCollector {
                file,
                min_statements,
                min_chars,
                collected: Vec::new(),
            };
            visitor.visit_program(parsed.program);
            collected.append(&mut visitor.collected);
        }

        if include_tokens {
            collect_token_fingerprints(file, self.min_tokens, min_chars, &mut collected);
        }

        ctx.corpus.with_slot(&DUPLICATE_BLOCKS, |slot| {
            slot.append(&mut collected);
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
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
                    location: Location::from_span(&fp.file, fp.span),
                    file: fp.file.clone(),
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
                location: Location::from_span(&primary.file, primary.span),
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
pub struct AstHashWalker {
    hasher: std::collections::hash_map::DefaultHasher,
    locals: HashMap<String, u32>,
    next_local: u32,
}

impl AstHashWalker {
    pub fn new() -> Self {
        Self {
            hasher: std::collections::hash_map::DefaultHasher::new(),
            locals: HashMap::new(),
            next_local: 0,
        }
    }

    fn tag(&mut self, bytes: &[u8]) {
        use std::hash::Hasher;
        self.hasher.write(bytes);
        self.hasher.write_u8(HASH_SEPARATOR);
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
        self.hasher.write_u8(HASH_SEPARATOR);
    }

    pub fn finish(self) -> u64 {
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
        self.hasher.write_u8(HASH_SEPARATOR);
    }

    fn visit_numeric_literal(&mut self, lit: &NumericLiteral<'a>) {
        use std::hash::Hasher;
        self.hasher.write(b"Num:");
        self.hasher.write(&lit.value.to_le_bytes());
        self.hasher.write_u8(HASH_SEPARATOR);
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
