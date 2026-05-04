//! Check trait + metadata.
//!
//! Five categories — the taxonomy is load-bearing: it's how users mentally
//! bucket findings, and downstream formatters group reports by category.
//! Configurable taxonomy (decision #8) lets projects *add* categories —
//! never remove these five.
//!
//! Note: as of cd-8wj this trait + the [`CheckContext`] it consumes live
//! in `cofferdam-core` only as the data-shaped portion of the contract.
//! The actual `Check` trait that built-in and plugin checks implement
//! lives in `cofferdam-ts` (or any future language adapter), where it
//! pairs the structures here with a parsed-AST view. Keeping `Check`
//! itself out of core is what lets core stay free of any oxc / language
//! coupling — see `design/platform-extensibility.md`.

use serde::{Deserialize, Serialize};

use crate::corpus::CorpusIndex;
use crate::issue::Severity;
use crate::options::OptionSpec;

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
    /// Optional file-scope filter (cd-81a.5). When `Some`, the engine
    /// pre-filters discovered files against this scope before invoking
    /// `run()` — saves the parse cost on inapplicable files. `None`
    /// (the default for built-ins) means the check runs on every file
    /// the engine sees.
    pub files: Option<&'static FileScope>,
}

/// Declarative file-scope filter for a check.
///
/// Plugin authors typically use `extensions` to constrain by file
/// extension (e.g. `&["ts", "tsx"]` for a JSX-aware check). `path_pattern`
/// and `exclude_patterns` use the standard ignore-crate glob syntax
/// (`**/resources/**`, `!**/dev_checks/**`).
#[derive(Debug, Clone, Copy)]
pub struct FileScope {
    /// File extensions (without leading dot) the check applies to. Empty
    /// slice = any extension.
    pub extensions: &'static [&'static str],
    /// Optional include glob — when `Some`, the file path must match.
    pub path_pattern: Option<&'static str>,
    /// Exclude globs — paths matching any of these are skipped, even if
    /// `path_pattern` matched.
    pub exclude_patterns: &'static [&'static str],
}

impl FileScope {
    pub const ANY: FileScope = FileScope {
        extensions: &[],
        path_pattern: None,
        exclude_patterns: &[],
    };
}

/// Context passed to `Check::finalize` (defined in language-adapter
/// crates such as `cofferdam-ts`). Carries the same `CorpusIndex` that
/// the per-file context shared, so cross-file checks can aggregate the
/// state they collected per file. Lives in core because `finalize` has
/// no current file or parsed AST — it operates only over the shared
/// run-scoped corpus, which is platform-agnostic.
pub struct FinalizeContext<'a> {
    pub corpus: &'a CorpusIndex,
}

impl<'a> FinalizeContext<'a> {
    pub fn new(corpus: &'a CorpusIndex) -> Self {
        Self { corpus }
    }
}
