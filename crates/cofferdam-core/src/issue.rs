//! Issue, priority, severity, and span types.
//!
//! Two axes, deliberately separate:
//! - **Priority** is *computed* — a derived score that sorts the report so
//!   the most actionable items surface first. Users do not configure it.
//! - **Severity** is *configured* — the lever that decides what fails CI.
//!   Users tune it per-check or per-category in `cofferdam.toml`.
//!
//! Splitting them lets users say "this is high-priority to fix but
//! shouldn't break the build yet" — a frequent ask on legacy codebases,
//! and what makes Baselines (decision #1) viable.

use serde::{Deserialize, Serialize};

/// Computed priority. Sort order in reports.
///
/// Rough scale:
/// - `>= 10`  : Higher
/// - `0..10`  : Normal
/// - `-10..0` : Low
/// - `< -10`  : Lower
///
/// `base_priority` on `CheckMeta` is the floor; the engine adjusts based on
/// file size, churn, and surrounding signals (phase 1+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Priority(pub i8);

impl Priority {
    pub const HIGHER: Priority = Priority(15);
    pub const NORMAL: Priority = Priority(5);
    pub const LOW: Priority = Priority(-5);
    pub const LOWER: Priority = Priority(-15);
}

/// Configured severity. CI gating only — never sort order.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational. Never fails CI.
    Info,
    /// Warning. Fails CI when `--strict` is set. Default when no override applies.
    #[default]
    Warning,
    /// Error. Always fails CI (non-zero exit).
    Error,
}

/// Source span, byte offsets into `SourceFile::text` plus 1-based line/col
/// for human display. Both are kept so formatters don't recompute and so
/// autofix (phase 3) can splice safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start_byte: u32,
    pub end_byte: u32,
    pub line: u32,
    pub column: u32,
}

/// One emitted finding.
///
/// `check_id` matches `CheckMeta.id` so reports can group + filter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub check_id: String,
    pub message: String,
    pub file: std::path::PathBuf,
    pub span: Span,
    pub priority: Priority,
    pub severity: Severity,
}
