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

/// Configured severity. Five-level axis used by `--fail-on=<level>` to
/// gate CI. Variant order is the severity order — `Ord` derives match
/// declaration order, so `severity >= threshold` works directly.
///
/// Severity is *configured* (via `cofferdam.toml` per-check overrides),
/// distinct from `Priority` (which is *computed* and only sorts the
/// report). Don't collapse the two — README design rule.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational. Never fails CI.
    Info,
    /// Low-impact finding (style, polish). Fails CI only at `--fail-on=low`.
    Low,
    /// Default. Most checks fall here. Fails CI at `--fail-on=medium` (the default).
    #[default]
    Medium,
    /// High-impact finding (likely correctness or design problem).
    High,
    /// Critical — almost always a real bug.
    Critical,
}

impl Severity {
    /// Lowercase string form, matching the serde `rename_all = "lowercase"`
    /// representation. Useful for formatters that don't want to round-trip
    /// through serde just to print the variant name.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// Parse a severity from a lowercase string. Used by the CLI's
    /// `--fail-on=<level>` parser and by `cofferdam.toml`'s per-check
    /// `severity = "..."` override.
    pub fn parse(s: &str) -> Result<Self, ParseSeverityError> {
        match s {
            "info" => Ok(Severity::Info),
            "low" => Ok(Severity::Low),
            "medium" => Ok(Severity::Medium),
            "high" => Ok(Severity::High),
            "critical" => Ok(Severity::Critical),
            other => Err(ParseSeverityError {
                input: other.to_string(),
            }),
        }
    }
}

/// Returned by `Severity::parse` when the input doesn't match a known
/// level. The error carries the offending input so the caller can
/// quote it back in a user-facing diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSeverityError {
    pub input: String,
}

impl std::fmt::Display for ParseSeverityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown severity `{}` (expected info | low | medium | high | critical)",
            self.input
        )
    }
}

impl std::error::Error for ParseSeverityError {}

impl std::str::FromStr for Severity {
    type Err = ParseSeverityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Severity::parse(s)
    }
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
///
/// `related` carries additional spans for findings that span multiple
/// locations (e.g. duplicate-block detection emits one issue per
/// duplicate set, with the canonical location in `span` and the other
/// occurrences in `related`). Empty for the common single-location case;
/// formatters omit it when empty.
///
/// `fix` carries an optional autofix payload (cd-81a.6). Plugins attach
/// fixes at report time via `ctx.report({ fix: ... })`; the fix engine
/// prefers the attached fix when present and falls back to the
/// `Check::autofix` trait method otherwise. `None` for issues without a
/// mechanical fix, omitted from JSON in that common case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub check_id: String,
    pub message: String,
    pub file: std::path::PathBuf,
    pub span: Span,
    pub priority: Priority,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<crate::edit::TextEdit>,
}

/// A span in another file related to the primary `Issue.span`. Carries
/// the file path (cross-file checks need it) and the same line/column
/// info `Span` does.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelatedSpan {
    pub file: std::path::PathBuf,
    pub span: Span,
}
