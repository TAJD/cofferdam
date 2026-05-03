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
pub struct FinalizeContext<'a> {
    pub corpus: &'a CorpusIndex,
}

impl<'a> FinalizeContext<'a> {
    pub fn new(corpus: &'a CorpusIndex) -> Self {
        Self { corpus }
    }
}

/// The check contract.
///
/// `run` is called once per file. `finalize` runs after all files have
/// been processed — that's where project-graph checks (decision #5) emit
/// their findings, e.g. orphaned exports, context-boundary violations,
/// or duplicate-block detection. `finalize` receives the shared
/// `CorpusIndex` it filled during `run` via `FinalizeContext`.
pub trait Check: Send + Sync {
    fn meta(&self) -> &'static CheckMeta;
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue>;
    fn finalize(&self, _ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
}
