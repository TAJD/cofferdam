//! Refactor checks — mechanical cleanups, often autofixable. The autofix
//! engine (phase 3) keys off this category for `--fix` defaults.

use cofferdam_core::{Category, Check, CheckContext, CheckMeta, Issue, SourceFile};

pub struct CognitiveComplexityStub;

const META: CheckMeta = CheckMeta {
    id: "Refactor.CognitiveComplexity",
    category: Category::Refactor,
    base_priority: 10,
    explanation: "Functions whose nesting + branching exceed the limit are hard to maintain.",
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for CognitiveComplexityStub {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }
    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
}
