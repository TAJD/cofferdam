//! Check trait + metadata.
//!
//! Five categories — the taxonomy is load-bearing: it's how users mentally
//! bucket findings, and downstream formatters group reports by category.
//! The five are **closed**: no project- or plugin-configurable additions.
//!
//! The real cross-domain extensibility surface lives in check IDs, not
//! categories. A SQL adapter's checks live in the existing buckets via
//! namespaced IDs (`Design.sql.foreign_key`, `Readability.sql.row_length`);
//! categories describe *intent* (architectural smell vs. readability nit),
//! namespaces describe *domain* (TypeScript vs. SQL vs. IaC). See cd-9hp.1
//! (predicate DSL) for the namespace forward-compat policy.
//!
//! (Earlier iterations of this comment pledged configurable categories;
//! that pledge is retracted — cd-9hp.3. If a real need for new top-level
//! categories appears later, reopen that bead and widen `Category` then.)

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::ast::AstView;
use crate::corpus::CorpusIndex;
use crate::edit::TextEdit;
use crate::issue::{Issue, Severity};
use crate::options::{CheckOptions, OptionSpec, EMPTY_OPTIONS};
use crate::source::{Language, SourceFile};

/// Process-wide empty corpus, used as a default when callers (mostly tests)
/// don't supply one. Lazily initialised because `CorpusIndex` is not const-
/// constructible (`HashMap::new` is not const).
fn empty_corpus() -> &'static CorpusIndex {
    static EMPTY: OnceLock<CorpusIndex> = OnceLock::new();
    EMPTY.get_or_init(CorpusIndex::default)
}

/// One of the five top-level finding categories. The taxonomy is
/// closed (no plugin- or project-configurable additions) and
/// load-bearing — formatters and the UI bucket findings by category.
/// See the module docs for the rationale and the cd-9hp.3 retraction
/// of the originally-pledged configurable-taxonomy surface.
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
    /// Whether this check supports mechanical autofix via `Check::autofix`.
    /// Set to `true` when the check's `autofix` override returns `Some(_)`
    /// for at least one class of finding. Defaults to `false` for forward-
    /// compatibility — new checks opt in explicitly. Used by `cofferdam fix
    /// --dry-run` to predict which findings would be modified and by the
    /// doc catalog / `explain` to surface support to users.
    pub autofix: bool,
    /// Whether `Check::run` is a pure function of `(file content, options)`.
    /// "Pure" here means the per-file `run()` body neither reads from nor
    /// writes to `CheckContext::corpus`, and produces a deterministic
    /// `Vec<Issue>` given the same inputs.
    ///
    /// The engine's per-file findings cache (cd-9hp.4 cp2) consults this
    /// flag: pure checks have their findings memoised under a
    /// `(content_hash, config_hash, engine_version)` key and replayed on
    /// cache hit, skipping the `run()` call entirely.
    ///
    /// Defaults to `false` — checks that touch corpus from `run()` (graph
    /// producers like `Design.DuplicateExportName`, scripted-invariant
    /// collectors) or that depend on the engine's per-run state must keep
    /// it `false`. Setting `true` when the check actually does touch
    /// corpus would mean cached findings get replayed without the
    /// corresponding side effects, silently dropping cross-file
    /// contributions.
    pub pure_run: bool,
}

/// Check IDs whose `finalize()` runs in the second finalize phase — after
/// every other check's `finalize()` has emitted and the
/// `ALL_PRE_FILTER_FINDINGS` corpus slot has been re-snapshotted to include
/// finalize-stage findings from the first phase. This lets the second-phase
/// observer see a complete picture of emitted findings, including those
/// from cross-file emitters (`Warning.UnusedImport`, `Design.OrphanExport`,
/// `Design.DeadExport`), before deciding which suppression directives are
/// stale.
///
/// Today only `Consistency.UnusedSuppression` qualifies. The engine
/// dispatches by check ID (cd-9hp.5) rather than via a `CheckMeta` flag —
/// the mechanism had one user in six months and the generic flag was paying
/// no rent. If a second observer use case appears, extend this set.
pub const FINALIZE_OBSERVER_CHECK_IDS: &[&str] = &["Consistency.UnusedSuppression"];

/// True when the check ID is in `FINALIZE_OBSERVER_CHECK_IDS` — engine helper
/// for the two-phase finalize dispatch.
pub fn is_finalize_observer(check_id: &str) -> bool {
    FINALIZE_OBSERVER_CHECK_IDS.contains(&check_id)
}

/// Base URL of the hosted cofferdam docs site. Matches the base used by
/// the README, `doctor.rs`, and `gen-docs`. A custom domain
/// (`cofferdam.dev`) redirects here; this is the codebase-canonical base.
pub const DOCS_BASE_URL: &str = "https://tajd.github.io/cofferdam";

/// Canonical docs-catalog URL for a check id (`Category.Name`). The
/// hosted catalog serves one clean-URL page per check at
/// `{DOCS_BASE_URL}/checks/{id}` (no `.html`, no trailing slash). The
/// id is used verbatim — case-sensitive, dot included — because that is
/// exactly the page slug `gen-docs` emits.
pub fn docs_url(check_id: &str) -> String {
    format!("{DOCS_BASE_URL}/checks/{check_id}")
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
    /// Per-language parsed handle, type-erased. The engine populates
    /// this with whatever type the matching language adapter produces
    /// (the Rust adapter installs a `&RustParseTree`; future SQL /
    /// IaC adapters install their own handles here). Checks downcast
    /// via [`CheckContext::parsed_as`].
    ///
    /// Stays `None` on `Language::TypeScript` files — TS checks read
    /// the oxc-typed `parsed` field directly. The Any-slot avoids
    /// cofferdam-core depending on per-language parser crates and
    /// keeps the polylingual surface generic over languages we
    /// haven't shipped yet (cd-0039).
    pub parsed_lang: Option<&'a dyn std::any::Any>,
    /// Resolved options for the running check, validated against its
    /// schema at engine startup. Defaults to a process-wide empty bag
    /// — useful for tests and for checks that declare no options.
    pub options: &'a CheckOptions,
    /// Run-scoped shared store for cross-file checks. Same instance
    /// passed into every per-file `CheckContext` and reused by
    /// `FinalizeContext`. Defaults to a process-wide empty corpus so
    /// per-file unit tests don't have to plumb one through.
    pub corpus: &'a CorpusIndex,
    /// Type oracle for checks declaring `CheckMeta::requires_types`
    /// (cd-9hp.2). `Some` only when the engine has a live type host —
    /// the CLI builds one when a type-aware check is registered and
    /// `[engine] type_aware` isn't disabled. The engine skips a
    /// `requires_types` check entirely when this is `None`, so a check
    /// that opted in can `unwrap()` it; defensive checks may still
    /// guard. `None` for every non-type-aware check.
    pub types: Option<&'a dyn crate::types::TypeOracle>,
}

impl<'a> CheckContext<'a> {
    /// Construct a minimal `CheckContext` borrowing `file`. The
    /// `parsed`, `parsed_lang`, `options`, and `corpus` slots use
    /// safe defaults — use the builder methods to install richer
    /// state. The engine builds a fully-populated context per file;
    /// tests typically only install the slots their check actually
    /// reads.
    pub fn new(file: &'a SourceFile) -> Self {
        Self {
            file,
            parsed: None,
            parsed_lang: None,
            options: &EMPTY_OPTIONS,
            corpus: empty_corpus(),
            types: None,
        }
    }

    /// Install the oxc-typed `ParsedView` for the current TS file.
    /// The engine calls this in its TS dispatch branch.
    pub fn with_parsed(mut self, parsed: &'a crate::parser::ParsedView<'a>) -> Self {
        self.parsed = Some(parsed);
        self
    }

    /// Install a per-language parsed handle. The engine calls this in
    /// its non-TS dispatch branch after the language adapter parses
    /// the file. Checks retrieve the value with [`Self::parsed_as`].
    pub fn with_parsed_lang(mut self, parsed_lang: &'a dyn std::any::Any) -> Self {
        self.parsed_lang = Some(parsed_lang);
        self
    }

    /// Install the validated `CheckOptions` bag for the running check.
    pub fn with_options(mut self, options: &'a CheckOptions) -> Self {
        self.options = options;
        self
    }

    /// Install the shared `CorpusIndex` used by cross-file checks.
    /// The engine creates one corpus per analysis run and shares it
    /// across every per-file context plus the finalize pass.
    pub fn with_corpus(mut self, corpus: &'a CorpusIndex) -> Self {
        self.corpus = corpus;
        self
    }

    /// Install the type oracle for this run (cd-9hp.2). The engine
    /// calls this in its TS dispatch branch only when an oracle is
    /// available, and only for checks declaring
    /// `CheckMeta::requires_types`.
    pub fn with_types(mut self, types: &'a dyn crate::types::TypeOracle) -> Self {
        self.types = Some(types);
        self
    }

    /// Typed accessor for `parsed_lang`. Returns `Some(&T)` when the
    /// installed handle's concrete type matches `T`. The Rust adapter
    /// reads `ctx.parsed_as::<RustParseTree>()`; the TS adapter does
    /// not use this method (it reads `ctx.parsed` directly).
    pub fn parsed_as<T: std::any::Any>(&self) -> Option<&'a T> {
        self.parsed_lang.and_then(|p| p.downcast_ref::<T>())
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
    /// Construct a `FinalizeContext` borrowing the run's shared
    /// `CorpusIndex`. The engine builds one per check and passes it
    /// to `Check::finalize`.
    pub fn new(corpus: &'a CorpusIndex) -> Self {
        Self {
            corpus,
            options: &EMPTY_OPTIONS,
        }
    }

    /// Install the validated `CheckOptions` bag for the running check.
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
    /// Source language this check targets. The engine consults this in
    /// its per-file loop and only invokes `run` / `pass2` / `autofix`
    /// when `file.language == self.language()`. `finalize` still fires
    /// unconditionally (cross-file checks read corpus slots, not files).
    ///
    /// Defaults to `Language::TypeScript` so every existing built-in
    /// compiles unchanged — only the Rust adapter's checks override.
    fn language(&self) -> Language {
        Language::TypeScript
    }
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

    /// Register this check's cross-file corpus slot(s) for per-file
    /// removal (cd-32 incremental analysis). Checks that accumulate a
    /// `Vec<E>` / `HashMap<PathBuf, _>` slot across files in `run()`
    /// and read it back in `finalize()` should override this to call
    /// `corpus.register_removable(&SLOT, |slot, path| ...)` once per
    /// slot they own — e.g. `slot.retain(|e| e.file != path)`.
    ///
    /// Called once when incremental-analysis state is built; the
    /// no-op default is correct for every check with no cross-file
    /// slot (the overwhelming majority).
    fn register_removable(&self, _corpus: &crate::CorpusIndex) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_url_produces_correct_url() {
        assert_eq!(
            docs_url("Warning.TripleEquals"),
            "https://tajd.github.io/cofferdam/checks/Warning.TripleEquals"
        );
    }
}
