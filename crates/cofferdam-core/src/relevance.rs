//! Shared relevance scale for `ContextItem.score` across every
//! `Context.*` provider (CD-210).
//!
//! Before this module existed, every provider picked its own score
//! numbers independently: `Context.Knowledge` used 100/50/10,
//! `Context.BlastRadius` computed a hop-decayed value peaking near
//! 100, `Context.Annotations` used 65/40, and `Context.Precedent`
//! used a deliberately low constant of 5 — with no documented
//! relationship between any of them. `context_digest::assemble` (in
//! `cofferdam-cli`) sorts every `ContextItem` from every provider by
//! `score` in one global pass to decide what survives `--budget`
//! truncation, so those independently-invented numbers were being
//! compared as if they meant the same thing.
//!
//! Every provider must map its own internal confidence onto one of
//! these four bands, using the anchor constant directly or scaling
//! within the band for finer-grained cases (e.g.
//! `Context.BlastRadius` decays within `DIRECT`/`INDIRECT` by hop
//! count). Bands are ordered by how directly the signal was observed,
//! not by which provider emits it.
//!
//! | Band | Anchor | Meaning |
//! |---|---|---|
//! | [`VERIFIED`] | 95 | Certain, directly observed: a finding on a changed line, a changed export called directly, a curated `priority: high` knowledge note. |
//! | [`DIRECT`] | 70 | A direct, one-hop relationship: a direct import, an annotation on the enclosing scope, a `priority: normal` knowledge note. |
//! | [`INDIRECT`] | 40 | Derived or one-more-hop-removed: a transitive importer, an annotation reached via an importer, legacy debt outside the diff. |
//! | [`INFERRED`] | 15 | A statistical/heuristic inference with real but modest confidence: a `priority: low` knowledge note. |
//!
//! [`FLOOR`] is reserved for `Context.Precedent`'s single constant
//! score, not a general-purpose low value — see its doc comment.

/// Certain, directly observed relevance. Anchor for the strongest
/// signal a provider can emit — a finding on a changed line, a
/// changed export called directly, a curated `priority: high`
/// knowledge note.
pub const VERIFIED: i32 = 95;

/// Direct, one-hop relevance — a directly observed relationship one
/// step removed from the change: a direct import, an annotation on
/// the file/scope being changed, a `priority: normal` knowledge note.
pub const DIRECT: i32 = 70;

/// Derived or one-more-hop-removed relevance: a transitive importer,
/// an annotation reached via an importer, legacy debt outside the
/// diff.
pub const INDIRECT: i32 = 40;

/// A statistical/heuristic inference with real but modest confidence:
/// a `priority: low` knowledge note.
pub const INFERRED: i32 = 15;

/// The minimum score any *decaying* provider signal may clamp down
/// to (e.g. `Context.BlastRadius`'s hop-count falloff). Deliberately
/// set strictly above [`FLOOR`] so a maximally-decayed graph-derived
/// signal never collides with `Context.Precedent`'s constant — see
/// [`FLOOR`]'s doc comment for why that ordering must hold.
pub const INFERRED_MIN: i32 = 6;

/// The absolute floor of the scale, reserved for `Context.Precedent`.
/// Per the product spec (wiki `cofferdam-context-product-criteria`),
/// precedent — a sibling-file convention *inference* rather than
/// anything traced through the import graph or authored by a human —
/// must rank below every other provider's signal, always. Pinning it
/// to a dedicated constant below [`INFERRED_MIN`] makes that ordering
/// hold by construction: no other provider may clamp this low.
///
/// This governs *presentation order and tie-breaks* (a `FLOOR`-scored
/// item always sorts last among items included in the same digest —
/// `crates/cofferdam-cli/src/context_digest.rs`'s `item_order`), not
/// eviction odds: `context_digest::assemble`'s round-robin fairness
/// (CD-211) guarantees every provider with items offered, including
/// `Context.Precedent`, a minimum share of the budget regardless of
/// score, so a `FLOOR`-scored item is not literally "first evicted"
/// under budget pressure (CD-217 — this was CD-210's original claim,
/// superseded once CD-211 landed fairness).
pub const FLOOR: i32 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    /// The scale must stay strictly ordered and `FLOOR` must stay
    /// strictly below `INFERRED_MIN` — that gap is what keeps
    /// `Context.Precedent`'s constant score from ever colliding with
    /// another provider's maximally-decayed signal (see `FLOOR`'s doc
    /// comment). If this ever fails, a constant was edited without
    /// re-reading why the ordering exists.
    #[test]
    fn bands_are_strictly_ordered_and_floor_stays_below_every_other_bound() {
        const {
            assert!(FLOOR < INFERRED_MIN);
            assert!(INFERRED_MIN <= INFERRED);
            assert!(INFERRED < INDIRECT);
            assert!(INDIRECT < DIRECT);
            assert!(DIRECT < VERIFIED);
        }
    }
}
