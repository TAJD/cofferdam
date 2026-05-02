//! Check trait + metadata.
//!
//! Five categories — the taxonomy is load-bearing: it's how users mentally
//! bucket findings, and downstream formatters group reports by category.
//! Configurable taxonomy (decision #8) lets projects *add* categories —
//! never remove these five.

use serde::{Deserialize, Serialize};

use crate::ast::AstView;
use crate::issue::Issue;
use crate::options::{CheckOptions, OptionSpec, EMPTY_OPTIONS};
use crate::source::SourceFile;

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
}

impl<'a> CheckContext<'a> {
    pub fn new(file: &'a SourceFile) -> Self {
        Self {
            file,
            parsed: None,
            options: &EMPTY_OPTIONS,
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
/// `run` is called once per file. `finalize` runs after all files have
/// been processed — that's where project-graph checks (decision #5) emit
/// their findings, e.g. orphaned exports or context-boundary violations.
pub trait Check: Send + Sync {
    fn meta(&self) -> &'static CheckMeta;
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue>;
    fn finalize(&self) -> Vec<Issue> {
        Vec::new()
    }
}
