//! Project graph — shared imports/exports tables for graph-aware checks.
//!
//! Pass 1 of the engine walks every file and records its imports and exports
//! into the well-known `CorpusKey<T>` slots defined here. Pass 2 / `finalize`
//! checks (orphan exports, import cycles, layer violations, dead exports,
//! project-aware unused imports) read from the same slots.
//!
//! Two checks that need the graph share the same keys; the engine populates
//! them once. The slots are kept dependency-light (plain data, no oxc refs)
//! so cofferdam-core stays parser-free. The engine's `graph.rs` does the
//! actual extraction.
//!
//! ## Orphan/cycle/layer checks read these
//!
//! * [`IMPORTS`] — every `import ... from '<specifier>'` (and dynamic-import
//!   bareword forms we can statically resolve) recorded with its resolved
//!   absolute path when the resolver succeeded.
//! * [`EXPORTS`] — every named export, default export, and re-export from
//!   a file we analyzed.
//!
//! ## Spec coverage
//!
//! Today: static `import` / `export` declarations. `require(...)` and dynamic
//! `import('...')` calls are not yet recorded — they'd add false-negatives
//! (orphan-export false positives) for codebases that mix static and dynamic
//! loading. Tracked as a follow-up; CommonJS-heavy projects shouldn't yet
//! rely on `Design.OrphanExport` and friends.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::corpus::CorpusKey;
use crate::issue::Span;

/// One static `import` (or `export ... from`) declaration in a source file.
///
/// `resolved` is `Some(absolute_path)` when the module resolver matched
/// the specifier to a file we discovered, and `None` for external bare
/// specifiers (`react`, `lodash`) or unresolvable paths. Graph-aware
/// checks should ignore unresolved imports — they mean "leaves the
/// project graph" — rather than treating them as errors.
#[derive(Debug, Clone)]
pub struct ImportRecord {
    /// Absolute path of the file containing the import declaration.
    pub from_file: PathBuf,
    /// Verbatim specifier from the source (`./foo`, `react`, `@/utils`).
    pub source_specifier: String,
    /// Resolved absolute path on disk, when the resolver succeeded.
    pub resolved: Option<PathBuf>,
    /// Names imported. Empty for side-effect-only imports
    /// (`import './polyfill';`).
    pub names: Vec<ImportedName>,
    /// `import type { ... } from '...'`. Per-specifier `type` modifiers
    /// are recorded on the individual `ImportedName` instead.
    pub type_only: bool,
    /// Span of the import statement itself, for reporting.
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    /// `import x from './m'`.
    Default,
    /// `import { x } from './m'` (with optional `as alias`).
    Named,
    /// `import * as x from './m'`.
    Namespace,
}

#[derive(Debug, Clone)]
pub struct ImportedName {
    /// Name as it appears in the source module (the export's name).
    /// For default imports this is the literal string `"default"`.
    /// For namespace imports this is the literal string `"*"`.
    pub source_name: String,
    /// Local binding name in the importing file.
    pub local_name: String,
    pub kind: ImportKind,
    /// `import { type X }` — per-specifier type-only flag.
    pub type_only: bool,
    /// Count of identifier references to `local_name` in the importing
    /// file, AFTER the import statement itself is excluded. Filled in
    /// pass 1 by the graph builder. Zero means the local binding is
    /// imported but never used — feeds `Refactor.DeadExport`.
    ///
    /// Includes value references AND type references (cofferdam-engine
    /// counts both `IdentifierReference` and `TSTypeName` hits since TS
    /// allows the same identifier to appear in either position).
    pub local_use_count: u32,
}

/// One export declaration from a source file.
///
/// Re-exports (`export { x } from './y'`) carry both the export name
/// (the alias the consumer sees) AND the source specifier — so a graph
/// consumer can walk the chain to the underlying definition when
/// computing dead-export reachability.
#[derive(Debug, Clone)]
pub struct ExportRecord {
    /// Absolute path of the exporting file.
    pub file: PathBuf,
    /// Name as exposed to consumers. For default exports this is
    /// `"default"`.
    pub name: String,
    pub kind: ExportKind,
    pub type_only: bool,
    /// Span of the exported identifier (or the `default` keyword for
    /// default exports), for reporting.
    pub span: Span,
    /// Set on `export { x } from './y'` — the verbatim specifier.
    pub source_specifier: Option<String>,
    /// Resolved absolute path of `source_specifier`, when the resolver
    /// succeeded.
    pub resolved_source: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// `export function foo`, `export const foo`, `export { foo }`.
    Named,
    /// `export default ...`.
    Default,
    /// `export { x } from './y'` or `export * from './y'`.
    /// (Star re-exports record `name = "*"`.)
    ReExport,
}

/// All static imports observed during pass 1.
pub static IMPORTS: CorpusKey<Vec<ImportRecord>> = CorpusKey::new("__cofferdam.graph.imports");

/// All named/default/re-export declarations observed during pass 1.
pub static EXPORTS: CorpusKey<Vec<ExportRecord>> = CorpusKey::new("__cofferdam.graph.exports");

/// Project-wide layer/architecture rules read from `cofferdam.toml`.
///
/// `Design.LayerViolation` reads this in `finalize`. The engine writes a
/// `Some` value here only when the loaded `ProjectConfig` had a
/// `[layers]` table; otherwise the slot stays `None` and the check
/// becomes a no-op.
///
/// We use `Option<LayersConfig>` (not `LayersConfig`) so the absence of
/// configuration is distinguishable from an empty config, which would
/// flag every cross-file edge.
#[derive(Debug, Clone, Default)]
pub struct LayersConfig {
    /// Absolute path of the directory containing `cofferdam.toml`.
    /// Glob patterns are matched against paths relative to this root.
    pub project_root: PathBuf,
    /// Layer name → list of glob patterns (gitignore-style). A file
    /// belongs to a layer when one of its globs matches the file's path
    /// relative to `project_root`.
    pub layers: BTreeMap<String, Vec<String>>,
    /// Layer name → list of layer names whose files this layer is
    /// allowed to import from. A layer always implicitly allows imports
    /// from itself; an entry of `[]` here means "isolated layer — must
    /// not import any other in-project file".
    pub allow: BTreeMap<String, Vec<String>>,
}

pub static LAYERS: CorpusKey<Option<LayersConfig>> = CorpusKey::new("__cofferdam.graph.layers");

/// Project-wide invariants from `cofferdam.invariants.toml`.
///
/// One runtime bundle — checks read whichever slice they care about
/// from a single `with_slot` call. `project_root` is the directory of
/// the loaded invariants.toml, used by checks to normalise globs and
/// project-relative paths.
#[derive(Debug, Clone, Default)]
pub struct InvariantsRuntime {
    pub project_root: PathBuf,
    pub public_api: crate::invariants::PublicApiSpec,
    pub boundaries: BTreeMap<String, crate::invariants::BoundarySpec>,
    pub invariants: BTreeMap<String, crate::invariants::InvariantSpec>,
}

impl InvariantsRuntime {
    pub fn is_empty(&self) -> bool {
        self.public_api.exports.is_empty()
            && self.boundaries.is_empty()
            && self.invariants.is_empty()
    }
}

/// Read by `Design.BoundaryFrozen`, `Design.InvariantViolation`, and
/// `Design.OrphanExport` (for its `[public_api]` allowlist). `None`
/// when no invariants spec was loaded — dependent checks become no-ops.
pub static INVARIANTS: CorpusKey<Option<InvariantsRuntime>> =
    CorpusKey::new("__cofferdam.graph.invariants");

/// Pre-suppression-filter findings indexed by file path.
///
/// The engine writes this slot in two phases (cd-wqc). First, all
/// `check.run()` and `pass2()` calls complete. Then Phase A of `finalize`
/// runs for every check NOT in `FINALIZE_OBSERVER_CHECK_IDS` (cross-file
/// emitters such as `Warning.UnusedImport`, `Design.OrphanExport`, and
/// `Design.DeadExport`). After Phase A, the slot is snapshotted with the
/// union of run/pass2 AND Phase A finalize findings so that Phase B observer
/// checks (today: only `Consistency.UnusedSuppression`) see a complete
/// picture.
///
/// Each entry is a list of `(check_id, 1-based-line)` pairs. The snapshot
/// includes finalize-stage findings from Phase A non-observer checks.
///
/// `Consistency.UnusedSuppression` reads this in its `finalize()` to
/// compare per-file inline suppressions against actual findings and flag
/// directives that suppressed nothing.
pub static ALL_PRE_FILTER_FINDINGS: CorpusKey<
    std::collections::HashMap<PathBuf, Vec<(String, u32)>>,
> = CorpusKey::new("__cofferdam.suppress.pre_filter_findings");

/// Set of check IDs registered in the current engine run.
///
/// The engine writes this at the start of each analysis run so that
/// `Consistency.UnusedSuppression` can distinguish stale-by-cause
/// suppressions (the finding was fixed — flag it) from stale-by-config
/// suppressions (the check isn't installed — don't flag it; that's
/// `Consistency.UnknownCheckId`'s territory).
pub static REGISTERED_CHECK_IDS: CorpusKey<std::collections::HashSet<String>> =
    CorpusKey::new("__cofferdam.suppress.registered_check_ids");
