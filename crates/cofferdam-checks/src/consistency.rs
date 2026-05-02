//! Consistency checks. Phase 2 wires up two-pass mode with `QuoteStyle` as
//! the canary; this stub exists so the engine has a Consistency entry to
//! group during phase 0.

use cofferdam_core::{Category, Check, CheckContext, CheckMeta, Issue, SourceFile};

pub struct QuoteStyleStub;

const META: CheckMeta = CheckMeta {
    id: "Consistency.QuoteStyle",
    category: Category::Consistency,
    base_priority: 0,
    explanation: "Mixed quote styles within a project hurt scanability.",
    requires_types: false,
    consistency: true,
    options: &[],
};

impl Check for QuoteStyleStub {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }
    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
}
