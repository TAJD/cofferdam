//! The `Language` trait — the abstraction the platform pivots on.
//!
//! Per `design/platform-extensibility.md`: cofferdam stays
//! oxc-/TypeScript-free in its platform layer (this crate), and any
//! number of language-adapter crates (`cofferdam-ts`, future
//! `cofferdam-py`) implement `Language`. The CLI binary picks one and
//! wires it in; the engine is generic over `L: Language` and never
//! mentions a parser or AST type by name.
//!
//! # Why a callback for `with_parsed` (vs `fn parse` returning a value)
//!
//! Most parsers — oxc included — produce an AST that borrows from a
//! per-file arena (an `Allocator` + a `Program<'a>` referencing it).
//! Returning the parsed view by value would require either
//!
//! - a self-referential struct (`self_cell`/`ouroboros` — extra dep,
//!   subtle UB risk in custom variants), or
//! - a long-lived storage type whose lifetime is threaded through the
//!   engine and every check (clutter, GAT lifetime gymnastics, hard to
//!   keep ergonomic).
//!
//! `with_parsed(file, |outcome| { ... checks for this file ... })` lets
//! the adapter create the arena on its stack, parse, run the closure
//! with a borrowed view, and drop the arena cleanly. The caller (engine)
//! sees a normal return value from the closure. This shape was the
//! cleanest of the three options when we tried each on cd-jub.
//!
//! # Parsed view: `Copy`
//!
//! `L::Parsed<'a>` is required to be `Copy`. The TypeScript adapter's
//! `ParsedView` is two references (`&Program`, `&[Diagnostic]`), which
//! is trivially Copy. Plugin authors who want a richer parsed view that
//! isn't Copy should wrap it in `&Self::Parsed<'a>` and store the
//! reference in `CheckContext.parsed` themselves — the `Copy` bound is
//! a deliberate ergonomic guarantee that lets `ctx.parsed` be a plain
//! `Option<L::Parsed<'a>>` field that checks can read repeatedly.

use crate::source::SourceFile;

/// Outcome of parsing one source file.
///
/// `parsed.is_some()` even when `diagnostics` is non-empty — non-fatal
/// parse errors still produce a usable AST and the engine runs checks
/// against it. `parsed.is_none()` only on fatal failure (the parser
/// gave up entirely); the engine emits `Warning.ParseError` from
/// `diagnostics` in that case.
pub struct ParseOutcome<P> {
    pub parsed: Option<P>,
    pub diagnostics: Vec<String>,
}

/// Language adapter — implemented by `cofferdam_ts::TypeScript` today,
/// by `cofferdam_py::Python` tomorrow, etc.
pub trait Language: 'static + Send + Sync {
    /// Borrowed parsed view exposed to checks via `CheckContext.parsed`.
    /// Must be `Copy` so `ctx.parsed` can be read repeatedly without
    /// move semantics — see the module docs.
    type Parsed<'a>: Copy + 'a
    where
        Self: 'a;

    /// Identifier used in logs / config / future per-language flags.
    /// `"typescript"` for the TS adapter; lowercase, ASCII.
    fn name() -> &'static str;

    /// Default file-extension set the engine discovers when no
    /// `--ext` override is set. e.g. `&["ts", "tsx", "mts", "cts"]`
    /// for TypeScript.
    fn default_extensions() -> &'static [&'static str];

    /// Parse `file` and call `f` with the parsed view + diagnostics.
    /// Adapter owns any per-file arena/allocator on its stack frame
    /// for the duration of the closure; checks may not retain
    /// references past the closure return.
    ///
    /// Returns whatever `f` returns — typically `Vec<Issue>`. The
    /// closure-based shape (vs returning the parsed view by value)
    /// avoids self-referential storage for parsers whose AST borrows
    /// from a per-file arena.
    fn with_parsed<R>(file: &SourceFile, f: impl FnOnce(ParseOutcome<Self::Parsed<'_>>) -> R) -> R;
}

/// Platform-purity guardrail (cd-jub acceptance + cd-7ws CI guard):
/// build a `Check`, run it via the engine seam, and emit an `Issue` —
/// without depending on any language adapter. If this doc test ever
/// needs `cofferdam-ts` (or any oxc crate) the platform/language split
/// has eroded.
///
/// Uses a stub language whose `Parsed<'a>` is `&'a str` and whose
/// `parse` returns the file text verbatim — enough to exercise the
/// generic engine seam without a real parser.
///
/// ```
/// use std::path::PathBuf;
/// use cofferdam_core::{
///     Category, Check, CheckContext, CheckMeta, Issue, Language, ParseOutcome,
///     Priority, Severity, SourceFile, Span,
/// };
///
/// // A stub adapter: "parse" is the identity over the source text.
/// // Doc tests run as a binary in `cofferdam-core`, so this is the
/// // proof that core can build + run a check end-to-end with no
/// // language-adapter crate in scope.
/// struct StubLang;
/// impl Language for StubLang {
///     type Parsed<'a> = &'a str;
///     fn name() -> &'static str { "stub" }
///     fn default_extensions() -> &'static [&'static str] { &["txt"] }
///     fn with_parsed<R>(
///         file: &SourceFile,
///         f: impl FnOnce(ParseOutcome<Self::Parsed<'_>>) -> R,
///     ) -> R {
///         f(ParseOutcome { parsed: Some(file.text.as_str()), diagnostics: vec![] })
///     }
/// }
///
/// // A check that flags lines containing "TODO".
/// struct TodoCheck;
/// const META: CheckMeta = CheckMeta {
///     id: "Stub.Todo",
///     category: Category::Warning,
///     base_priority: 5,
///     default_severity: Severity::Medium,
///     explanation: "flag TODO comments",
///     body: "",
///     requires_types: false,
///     consistency: false,
///     options: &[],
///     files: None,
/// };
/// impl Check<StubLang> for TodoCheck {
///     fn meta(&self) -> &'static CheckMeta { &META }
///     fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_, '_, StubLang>) -> Vec<Issue> {
///         let Some(text) = ctx.parsed else { return vec![] };
///         text.lines()
///             .enumerate()
///             .filter(|(_, l)| l.contains("TODO"))
///             .map(|(i, _)| Issue {
///                 check_id: META.id.to_string(),
///                 message: "TODO found".into(),
///                 file: file.path.clone(),
///                 span: Span { line: (i + 1) as u32, column: 1, start_byte: 0, end_byte: 0 },
///                 priority: Priority(META.base_priority),
///                 severity: Severity::Medium,
///                 related: vec![],
///                 fix: None,
///             })
///             .collect()
///     }
/// }
///
/// // Drive the seam directly — no engine, no formatter, just core.
/// let file = SourceFile::new(PathBuf::from("a.txt"), "hello\nTODO fix\n".to_string());
/// let issues = StubLang::with_parsed(&file, |outcome| {
///     let mut ctx = CheckContext::<StubLang>::new(&file);
///     ctx.parsed = outcome.parsed;
///     TodoCheck.run(&file, &mut ctx)
/// });
/// assert_eq!(issues.len(), 1);
/// assert_eq!(issues[0].check_id, "Stub.Todo");
/// assert_eq!(issues[0].span.line, 2);
/// ```
pub fn _platform_purity_doc_test() {}
