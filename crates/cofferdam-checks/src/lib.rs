//! `cofferdam-checks` — built-in checks shipped with the binary.
//!
//! Phase 0 ships exactly one real check (`Readability.MaxLineLength`) plus
//! one stub per remaining category to validate that the engine groups and
//! sorts across all five categories. Real implementations land
//! progressively as oxc and the project graph wire up.

pub mod consistency;
pub mod design;
pub mod framework_paths;
pub mod readability;
pub mod refactor;
pub mod warning;

use cofferdam_core::{Check, LineView};

/// True for lines that contribute to "effective" length: non-blank and not
/// a pure comment line. A line counts as a pure comment only when oxc's
/// comment table flags it AND the trimmed text starts with `//`, `/*`, or
/// `*` — that excludes JSDoc continuations and `*/` closers but keeps
/// trailing inline comments on otherwise-code lines (`x++; // bump`) as
/// code.
pub(crate) fn is_code_line(line: &LineView<'_>) -> bool {
    let t = line.text.trim();
    if t.is_empty() {
        return false;
    }
    if line.is_comment && (t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')) {
        return false;
    }
    true
}

/// Count of skippable (blank or pure-comment) lines whose 1-based line
/// numbers fall in `[lo, hi]` inclusive. Saturates on empty / inverted
/// ranges. Used by length-aware checks to discount non-code from the raw
/// line-span delta returned by `span_from_bytes`.
pub(crate) fn count_skippable_lines(views: &[LineView<'_>], lo: u32, hi: u32) -> u32 {
    if lo > hi || lo == 0 {
        return 0;
    }
    let lo_idx = (lo as usize).saturating_sub(1);
    let hi_idx = (hi as usize).min(views.len());
    if lo_idx >= hi_idx {
        return 0;
    }
    views[lo_idx..hi_idx]
        .iter()
        .filter(|l| !is_code_line(l))
        .count() as u32
}

/// All built-in checks, ready for the engine to consume.
///
/// Phase 0 returns five entries — one per category. Phase 1+ adds the rest.
/// The Rust adapter's checks (cd-91zc) are appended at the tail — the
/// engine's per-language dispatch (`Check::language()` vs
/// `SourceFile.language`) ensures they only fire on `.rs` files.
pub fn all_builtins() -> Vec<Box<dyn Check>> {
    let mut checks: Vec<Box<dyn Check>> = vec![
        Box::new(readability::MaxLineLength::new(120)),
        Box::new(readability::MaxFunctionLength::new(50)),
        Box::new(consistency::QuoteStyle),
        Box::new(consistency::BroadSuppression),
        Box::new(consistency::UnusedSuppression),
        Box::new(design::MaxParameters::new(5)),
        Box::new(design::DuplicateExportName),
        Box::new(design::OrphanExport),
        Box::new(design::ImportCycle),
        Box::new(design::LayerViolation),
        Box::new(design::BoundaryFrozen),
        Box::new(design::InvariantViolation),
        Box::new(design::ScriptedInvariant),
        Box::new(design::UnionExhaustivenessGap),
        Box::new(refactor::CyclomaticComplexity::new(10)),
        Box::new(refactor::CognitiveComplexity::new(15)),
        Box::new(refactor::LongAndComplex::new(75, 15)),
        Box::new(refactor::DuplicateBlock::default()),
        Box::new(refactor::PreferOptionalChain),
        Box::new(refactor::DeadExport),
        Box::new(refactor::PreferNullishCoalescing),
        Box::new(refactor::UnusedVariable),
        Box::new(refactor::MutatedParameter),
        Box::new(warning::TripleEquals),
        Box::new(warning::UnusedImport),
        Box::new(warning::UnusedNullCheck),
        Box::new(warning::NoConsoleLog),
        Box::new(warning::NoDebugger),
        Box::new(warning::NoEval),
    ];
    checks.extend(cofferdam_rust::all_rust_checks());
    checks.extend(cofferdam_html::all_html_checks());
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    /// `Warning.TripleEquals` is the only builtin with a real `autofix`
    /// implementation today. Confirm it is the sole check with
    /// `meta().autofix == true`, and that every other check is `false`.
    #[test]
    fn meta_autofix_field_is_populated() {
        let builtins = all_builtins();
        let autofix_true: Vec<&str> = builtins
            .iter()
            .filter(|c| c.meta().autofix)
            .map(|c| c.meta().id)
            .collect();
        assert_eq!(
            autofix_true,
            vec!["Warning.TripleEquals"],
            "unexpected set of checks with autofix=true: {:?}",
            autofix_true
        );

        // All remaining checks must be false.
        let autofix_false_count = builtins.iter().filter(|c| !c.meta().autofix).count();
        assert_eq!(
            autofix_false_count,
            builtins.len() - 1,
            "expected all checks except Warning.TripleEquals to have autofix=false"
        );
    }

    /// Every file in `docs/` must correspond to a registered builtin's id.
    /// `include_str!` already enforces the reverse direction (each builtin's
    /// `META.body` references an existing file at compile time).
    #[test]
    fn no_orphan_companion_files() {
        let registered: HashSet<String> = all_builtins()
            .iter()
            .map(|c| format!("{}.md", c.meta().id))
            .collect();

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
        let on_disk: HashSet<String> = fs::read_dir(&dir)
            .expect("failed to read docs/ directory")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md"))
            .collect();

        let orphans: Vec<&String> = on_disk.difference(&registered).collect();
        assert!(
            orphans.is_empty(),
            "orphan companion files in crates/cofferdam-checks/docs/ (no matching check in all_builtins()): {:?}",
            orphans
        );
    }
}
