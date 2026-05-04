//! Check trait + metadata.
//!
//! Five categories — the taxonomy is load-bearing: it's how users mentally
//! bucket findings, and downstream formatters group reports by category.
//! Configurable taxonomy (decision #8) lets projects *add* categories —
//! never remove these five.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::ast::AstView;
use crate::corpus::CorpusIndex;
use crate::edit::TextEdit;
use crate::issue::{Issue, Severity};
use crate::options::{CheckOptions, OptionSpec, EMPTY_OPTIONS};
use crate::source::SourceFile;

/// Process-wide empty corpus, used as a default when callers (mostly tests)
/// don't supply one. Lazily initialised because `CorpusIndex` is not const-
/// constructible (`HashMap::new` is not const).
fn empty_corpus() -> &'static CorpusIndex {
    static EMPTY: OnceLock<CorpusIndex> = OnceLock::new();
    EMPTY.get_or_init(CorpusIndex::default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Style invariants enforced across the project. Two-pass: pass 1
    /// learns the dominant style; pass 2 flags deviations.
    Consistency,
    /// Architectural smells — boundary violations, orphaned exports,
    /// coupling. Often type- or graph-aware.
    Design,
    /// Surface-level legibility — naming, line length, comment quality.
    Readability,
    /// Mechanical cleanups — dead code, redundant nesting, complexity.
    /// Most autofixable checks live here.
    Refactor,
    /// Likely bugs or footguns — `==` vs `===`, unhandled rejections,
    /// always-truthy conditions. Highest default severity.
    Warning,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Consistency,
        Category::Design,
        Category::Readability,
        Category::Refactor,
        Category::Warning,
    ];
}

/// Static metadata for a check. One per `impl Check`, returned from
/// `Check::meta()` as `&'static`.
#[derive(Debug, Clone, Copy)]
pub struct CheckMeta {
    /// Dotted ID, `Category.Name`. Stable string used in config, baseline
    /// files, suppression comments, and SARIF rule IDs. Never rename
    /// without a deprecation window.
    pub id: &'static str,
    pub category: Category,
    /// Floor for the priority computation. Range -20..=20.
    pub base_priority: i8,
    /// Default severity for findings emitted by this check. Users
    /// override per-check via `[checks."X.Y"] severity = "..."` in
    /// `cofferdam.toml`. The engine assigns this (or the override) to
    /// every emitted `Issue.severity` in a post-pass — checks don't
    /// need to set it themselves.
    pub default_severity: Severity,
    pub explanation: &'static str,
    /// Long-form catalog body — extracted to a companion markdown file
    /// at `crates/cofferdam-checks/docs/<id>.md` and pulled in via
    /// `include_str!` so the file's existence is enforced at compile
    /// time. Used by `cofferdam explain --full`, the gen-docs catalog
    /// (Phase 2), and the published VitePress site. Includes the
    /// frontmatter prologue verbatim — gen-docs splits it; in-terminal
    /// renderers strip it.
    pub body: &'static str,
    /// Type-aware checks (decision #4) — engine routes these to the
    /// ts-morph worker pool instead of the Rust pipeline.
    pub requires_types: bool,
    /// Two-pass consistency mode (phase 2 canary). Engine collects
    /// per-file evidence in pass 1, then asks the check to flag
    /// deviations in pass 2.
    pub consistency: bool,
    /// Per-check options schema. Engine validates user config against
    /// this and lends the resolved values to `CheckContext::options`
    /// for every file. `&[]` means the check takes no options.
    pub options: &'static [OptionSpec],
}

/// Mutable per-file scratch passed to `Check::run`. Carries the
/// SourceFile and (when available) the parsed AST.
///
/// `parsed` is `None` only when the check declared no AST need (today:
/// none — but the field is plumbed so phase-1+ checks can opt out for
/// raw-text scans) or when parsing produced no usable Program. Checks
/// that need the AST should treat `None` as "skip this file" rather
/// than panicking.
pub struct CheckContext<'a> {
    pub file: &'a SourceFile,
    pub parsed: Option<&'a crate::parser::ParsedView<'a>>,
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

    pub fn with_parsed(mut self, parsed: &'a crate::parser::ParsedView<'a>) -> Self {
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

/// Context passed to `Check::finalize`. Carries the same `CorpusIndex`
/// that ran-phase `CheckContext` shared, so cross-file checks can
/// aggregate the state they collected per file. Deliberately distinct
/// from `CheckContext` because `finalize` has no current file or parsed
/// AST.
///
/// `options` carries the running check's resolved options, mirroring
/// `CheckContext::options` (cd-3uj). The engine pairs each check with
/// its slot when calling `finalize` so cross-file checks can honour
/// `cofferdam.toml` overrides for the same option keys exposed in
/// per-file `run`.
pub struct FinalizeContext<'a> {
    pub corpus: &'a CorpusIndex,
    /// Resolved options for the running check. Defaults to a process-
    /// wide empty bag — useful for tests and for checks that declare
    /// no options.
    pub options: &'a CheckOptions,
}

impl<'a> FinalizeContext<'a> {
    pub fn new(corpus: &'a CorpusIndex) -> Self {
        Self {
            corpus,
            options: &EMPTY_OPTIONS,
        }
    }

    pub fn with_options(mut self, options: &'a CheckOptions) -> Self {
        self.options = options;
        self
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
