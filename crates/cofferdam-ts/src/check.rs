//! TS-flavored `Check` trait and the `CheckContext` flavor that carries
//! the parsed AST.
//!
//! Lives here (not in `cofferdam-core`) because both the trait and the
//! context-shape reference [`crate::ParsedView`], which holds an oxc
//! `Program`. Keeping them out of core is what lets core stay
//! oxc-free — see `design/platform-extensibility.md`.
//!
//! Future cd-jub: a `Language` trait moves into `cofferdam-core` and
//! generalises this trait to `Check<L: Language>`. The structural shape
//! here was chosen to make that migration mechanical: `CheckContext`
//! already holds only `file`, `parsed`, `options`, `corpus`, in the
//! same order the generic version will use.

use std::sync::OnceLock;

use cofferdam_core::{
    CheckMeta, CheckOptions, CorpusIndex, FinalizeContext, Issue, SourceFile, TextEdit,
    EMPTY_OPTIONS,
};

use crate::ast::AstView;
use crate::parser::ParsedView;

/// Process-wide empty corpus, used as a default when callers (mostly
/// tests) don't supply one. Lazily initialised because `CorpusIndex`
/// is not const-constructible (`HashMap::new` is not const).
fn empty_corpus() -> &'static CorpusIndex {
    static EMPTY: OnceLock<CorpusIndex> = OnceLock::new();
    EMPTY.get_or_init(CorpusIndex::default)
}

/// Mutable per-file scratch passed to `Check::run`. Carries the
/// SourceFile and (when available) the parsed AST.
///
/// `parsed` is `None` only when parsing produced no usable Program. Checks
/// that need the AST should treat `None` as "skip this file" rather
/// than panicking.
pub struct CheckContext<'a> {
    pub file: &'a SourceFile,
    pub parsed: Option<&'a ParsedView<'a>>,
    /// Resolved options for the running check, validated against its
    /// schema at engine startup. Defaults to a process-wide empty bag
    /// — useful for tests and for checks that declare no options.
    pub options: &'a CheckOptions,
    /// Run-scoped shared store for cross-file checks. Same instance
    /// passed into every per-file `CheckContext` and reused by
    /// `FinalizeContext`. Defaults to a process-wide empty corpus so
    /// per-file unit tests don't have to plumb one through.
    pub corpus: &'a CorpusIndex,
}

impl<'a> CheckContext<'a> {
    pub fn new(file: &'a SourceFile) -> Self {
        Self {
            file,
            parsed: None,
            options: &EMPTY_OPTIONS,
            corpus: empty_corpus(),
        }
    }

    pub fn with_parsed(mut self, parsed: &'a ParsedView<'a>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_options(mut self, options: &'a CheckOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_corpus(mut self, corpus: &'a CorpusIndex) -> Self {
        self.corpus = corpus;
        self
    }

    /// Plugin-facing AST surface. `None` when the file failed to parse
    /// (engine emitted `Warning.ParseError` for those). Built-in checks
    /// may continue to use `ctx.parsed` directly with `oxc_ast_visit`;
    /// this method is the layered, stable surface used by plugins.
    pub fn ast(&self) -> Option<AstView<'a>> {
        self.parsed
            .map(|p| AstView::new(p.program, &self.file.text))
    }
}

/// The check contract.
///
/// `run` is called once per file. `pass2` is called once per file AFTER
/// all checks' `run` has completed for every file — this is the two-pass
/// consistency mode. `finalize` runs after all files have been processed
/// (including pass 2) — that's where project-graph checks (decision #5)
/// emit their findings, e.g. orphaned exports, context-boundary
/// violations, or duplicate-block detection. `finalize` receives the
/// shared `CorpusIndex` it filled during `run` via `FinalizeContext`.
///
/// `autofix` is called by the fix engine for each issue emitted by this
/// check. Returning `Some(TextEdit)` opts the check into mechanical
/// autofix; returning `None` (the default) opts out. The fix engine skips
/// issues whose check returns `None` and processes the rest in reverse
/// byte-offset order to avoid span-shift bugs.
pub trait Check: Send + Sync {
    fn meta(&self) -> &'static CheckMeta;
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue>;
    /// Per-file second pass. Runs AFTER all checks' first-pass `run`
    /// completes for every file. Use when a check needs to see all files
    /// before emitting findings (e.g. "the dominant quote style in this
    /// file is X, flag deviations"). Reads evidence collected in pass 1
    /// via `ctx.corpus`. Returns findings the same way `run` does.
    ///
    /// Only called for checks whose `meta().consistency == true`.
    fn pass2(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
    fn finalize(&self, _ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
    /// Return the `TextEdit` that would fix `issue`, or `None` if this
    /// check does not support mechanical autofix for the given finding.
    ///
    /// The `source` parameter carries the raw text of the file that
    /// produced `issue`, allowing the implementation to inspect the
    /// bytes at `issue.span` without re-reading from disk.
    fn autofix(&self, _issue: &Issue, _source: &SourceFile) -> Option<TextEdit> {
        None
    }
}
