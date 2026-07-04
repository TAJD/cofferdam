//! `TypstCheck` — the dedicated check abstraction for this crate.
//!
//! Deliberately NOT `cofferdam_core::Check`: that trait is shaped around
//! per-file AST analysis (`requires_types`, `consistency`, `pure_run`,
//! `autofix`, `options`, an `include_str!` markdown body) which has no
//! meaning for a package-directory-level check. `TypstCheckMeta` keeps
//! only what actually applies here; findings still reuse
//! `cofferdam_core::Issue` so output shape matches TS/Rust output.

use cofferdam_core::{Category, Issue, Priority, Severity};

use crate::package::TypstPackage;

/// Static metadata for a Typst check. IDs are namespaced `Typst.<Name>`
/// into the five existing `Category` variants.
pub struct TypstCheckMeta {
    pub id: &'static str,
    pub category: Category,
    pub default_severity: Severity,
    pub explanation: &'static str,
}

/// The check contract: given a loaded package, return zero or more
/// findings. No per-file loop, no AST — the whole package directory is
/// the unit of analysis.
pub trait TypstCheck: Send + Sync {
    fn meta(&self) -> &'static TypstCheckMeta;
    fn check(&self, pkg: &TypstPackage) -> Vec<Issue>;
}

/// `TypstCheckMeta` has no `base_priority` field (unlike
/// `cofferdam_core::CheckMeta`) — there's no per-file engine sort to
/// feed. Priority is derived from severity so `Issue.priority` still
/// sorts sensibly if a caller renders through the shared formatters.
pub fn priority_for_severity(sev: Severity) -> Priority {
    match sev {
        Severity::Critical => Priority(20),
        Severity::High => Priority::HIGHER,
        Severity::Medium => Priority::NORMAL,
        Severity::Low => Priority::LOW,
        Severity::Info => Priority::LOWER,
    }
}

/// Every built-in Typst check, mirroring `cofferdam_checks::all_builtins()`.
pub fn all_typst_checks() -> Vec<Box<dyn TypstCheck>> {
    use crate::checks::*;
    vec![
        Box::new(ManifestRequiredFields),
        Box::new(PackageNameNotCanonical),
        Box::new(PackageNameKebabCase),
        Box::new(DescriptionStyle),
        Box::new(ManifestVersionMatchesDir),
        Box::new(ReadmeVersionMatchesManifest),
        Box::new(LicenseFileMatchesSpdx),
        Box::new(RelativeImportInPublishedReadme),
        Box::new(BundleIncludesPdf),
        Box::new(LicenseMissing),
        Box::new(ChangelogMissing),
    ]
}
