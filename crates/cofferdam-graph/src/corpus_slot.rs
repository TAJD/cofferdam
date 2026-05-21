//! Corpus-slot wiring for the canonical graph.
//!
//! The engine populates [`CANONICAL_GRAPH`] once after pass 1 (and
//! pass 2 consistency checks) finishes; finalize-stage checks read
//! the slot to run graph queries rather than joining flat-table
//! `IMPORTS` / `EXPORTS` records themselves.
//!
//! cd-9hp.9 cp3 is the first writer; readers migrate one at a time
//! off the legacy slots in [`cofferdam_core::graph`]
//! (`Design.OrphanExport` first; `DeadExport`, `ImportCycle`,
//! `LayerViolation` follow in subsequent checkpoints).

use cofferdam_core::CorpusKey;

use crate::store::Graph;

/// Engine-populated canonical-graph slot.
///
/// One [`Graph`] per analysis run, materialised from the per-file
/// `IMPORTS` / `EXPORTS` evidence via
/// [`crate::build::build_canonical_graph`]. Finalize-stage checks
/// query it through the standard
/// [`cofferdam_core::CorpusIndex::with_slot`] entry point.
pub static CANONICAL_GRAPH: CorpusKey<Graph> = CorpusKey::new("__cofferdam.graph.canonical");
