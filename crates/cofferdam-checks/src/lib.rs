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

use cofferdam_core::Check;

/// All built-in checks, ready for the engine to consume.
///
/// Phase 0 returns five entries — one per category. Phase 1+ adds the rest.
pub fn all_builtins() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(readability::MaxLineLength::new(120)),
        Box::new(readability::MaxFunctionLength::new(50)),
        Box::new(consistency::QuoteStyleStub),
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
