//! `cofferdam-graph` — canonical graph schema for cross-file rules.
//!
//! This crate hosts the **type definitions** for the canonical graph
//! that cd-9hp.9 puts under cofferdam's cross-file analyses. The
//! storage backend (petgraph + indexed attribute maps) and the
//! mutation/query API land in checkpoint 2 inside this same crate;
//! checkpoint 1 is type-only so the schema lives as a stand-alone
//! contract that the engine, checks, formatters, and the predicate
//! DSL can compile against without pulling in a graph backend.
//!
//! # Design references
//!
//! - [docs/decisions/0001-canonical-graph-schema.md] — ADR pinning
//!   the **closed core + namespaced extensions** taxonomy and the
//!   **petgraph + indexed attribute maps** storage backend.
//! - cd-9hp.9 design notes — full motivation, sequencing, and the
//!   measurements from the cd-acv3 spike.
//! - cd-9hp.1 — predicate DSL whose v1 grammar reserves the
//!   `transitively imports`, `imports as type` / `imports as value`,
//!   and `core.*` / `ts.*` namespace surfaces that v2 will compile
//!   against this schema.
//!
//! # Crate layout
//!
//! - [`schema`] — `NodeKind`, `EdgeKind`, `SymbolKind`, `ExportKind`.
//!   The closed core; adapter-specific types ride on
//!   [`schema::NodeKind::Extension`] / [`schema::EdgeKind::Extension`].
//! - [`value`] — `Value` and `AttrMap`. JSON-roundtrippable attribute
//!   payload carried by `Extension` variants and by per-edge metadata
//!   (e.g. import specifier, layer name).
//! - [`id`] — `NodeId(u64)`. Stable identity computed from
//!   `(variant discriminant, identifying attrs)` via sha2 truncated
//!   to 64 bits. Stable within and across processes for a given
//!   build of cofferdam-graph.
//! - [`store`] — petgraph-backed [`store::Graph`] with path and
//!   layer indexes and bounded-depth BFS over edge-kind filters.
//!   The mutation API is build-only at cp2; incremental removal /
//!   replacement lands with cd-9hp.4.
//!
//! # What this crate is NOT (yet)
//!
//! - No `schema_version` enforcement at load time — cp5 wires the
//!   versioning policy through.

pub mod build;
pub mod corpus_slot;
pub mod id;
pub mod schema;
pub mod store;
pub mod value;

pub use build::{build_canonical_graph, normalized_file_path};
pub use corpus_slot::CANONICAL_GRAPH;
pub use id::{compute_node_id, NodeId};
pub use schema::{EdgeKind, ExportKind, NodeKind, SymbolKind};
pub use store::{EdgePayload, Graph};
pub use value::{AttrMap, Value};

/// Schema version this build emits and reads natively. Bumps follow
/// the MAJOR.MINOR policy in `docs/schema-versioning.md`. Surfaced
/// here so the engine and the predicate DSL can pin against a single
/// constant; full load-time enforcement lands in cd-9hp.9 cp5.
///
/// Re-exported from [`cofferdam_core::invariants`] so a single
/// canonical type is used across the invariants spec, the DSL
/// grammar, and the graph schema.
pub const SCHEMA_VERSION: cofferdam_core::invariants::SchemaVersion =
    cofferdam_core::invariants::SchemaVersion::new(1, 0);
