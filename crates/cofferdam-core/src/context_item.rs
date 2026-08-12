//! `ContextItem` — the advisory payload emitted by Context-category
//! checks ("providers"). Deliberately NOT an `Issue`: no span
//! requirement, no severity, never baselined, never gates CI. The
//! serialized form is a public additive-only contract (exposed via
//! `cofferdam context --format json`).

use crate::issue::RelatedSpan;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Emitting provider's dotted id, e.g. `Context.BlastRadius`.
    pub check_id: String,
    /// One-line heading shown in the digest.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Relevance — higher sorts earlier. Compared directly against
    /// every other provider's `score` by `context_digest::assemble`
    /// (in `cofferdam-cli`) to decide what survives `--budget`
    /// truncation, so it is NOT provider-relative: every provider
    /// must map its own confidence onto the shared scale in
    /// [`crate::relevance`] (CD-210) rather than inventing its own
    /// numbers.
    pub score: i32,
    /// Pinned items are never evicted by budget truncation
    /// (findings summaries, high-priority knowledge notes).
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedSpan>,
    /// Why this fired (selector, edge path) — populated for `--explain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_item_serde_omits_empty_optional_fields() {
        let item = ContextItem {
            check_id: "Context.Knowledge".into(),
            title: "Billing invariants".into(),
            body: "No rounding.".into(),
            score: 100,
            pinned: true,
            related: vec![],
            explain: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(!json.contains("related"));
        assert!(!json.contains("explain"));
        let back: ContextItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "Billing invariants");
    }
}
