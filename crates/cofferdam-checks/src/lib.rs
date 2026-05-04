//! `cofferdam-checks` — built-in checks shipped with the binary.
//!
//! Phase 0 ships exactly one real check (`Readability.MaxLineLength`) plus
//! one stub per remaining category to validate that the engine groups and
//! sorts across all five categories. Real implementations land
//! progressively as oxc and the project graph wire up.

pub mod consistency;
pub mod design;
pub mod readability;
pub mod refactor;
pub mod warning;

use cofferdam_ts::Check;

/// All built-in checks, ready for the engine to consume.
///
/// Phase 0 returns five entries — one per category. Phase 1+ adds the rest.
pub fn all_builtins() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(readability::MaxLineLength::new(120)),
        Box::new(readability::MaxFunctionLength::new(50)),
        Box::new(consistency::QuoteStyle),
        Box::new(design::MaxParameters::new(5)),
        Box::new(design::DuplicateExportName),
        Box::new(refactor::CyclomaticComplexity::new(10)),
        Box::new(refactor::CognitiveComplexity::new(15)),
        Box::new(refactor::DuplicateBlock::default()),
        Box::new(refactor::PreferOptionalChain),
        Box::new(refactor::PreferNullishCoalescing),
        Box::new(refactor::UnusedVariable),
        Box::new(warning::TripleEquals),
        Box::new(warning::NoConsoleLog),
        Box::new(warning::NoDebugger),
        Box::new(warning::NoEval),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

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
