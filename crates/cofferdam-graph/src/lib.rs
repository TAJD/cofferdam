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
//!   `add_node_owned`/`add_edge_owned`/`remove_file` (cd-36) give
//!   nodes and edges per-file provenance and let a changed file's
//!   contributions be dropped without a full rebuild.
//!
//! # Schema versioning
//!
//! [`CURRENT_SCHEMA_VERSION`] / [`MIN_SUPPORTED_SCHEMA_VERSION`] pin
//! the canonical graph's `MAJOR.MINOR` policy (see
//! `docs/schema-versioning.md`). The [`loader`] module wires
//! load-time enforcement against serialised graph bundles: a JSON
//! envelope with a reserved `schema_version` key is validated against
//! the same matrix `cofferdam.invariants.toml` already enforces
//! (cd-9hp.12). Adapter (cd-9hp.10) and incremental-cache (cd-9hp.4)
//! formats will sit inside that envelope.

pub mod build;
pub mod corpus_slot;
pub mod id;
pub mod loader;
pub mod schema;
pub mod store;
pub mod value;

pub use build::{apply_records, build_canonical_graph, normalized_file_path};
pub use corpus_slot::CANONICAL_GRAPH;
pub use id::{compute_node_id, NodeId};
pub use loader::{
    load_envelope, validate_envelope_version, validate_envelope_version_against, GraphEnvelope,
    LoadError,
};
pub use schema::{EdgeKind, ExportKind, NodeKind, SymbolKind};
pub use store::{EdgePayload, Graph};
pub use value::{AttrMap, Value};

/// Schema version this build of cofferdam emits and reads natively.
///
/// Bumps follow the MAJOR.MINOR policy in
/// `docs/schema-versioning.md`. Re-exported from
/// [`cofferdam_core::invariants`] so a single canonical type is used
/// across the invariants spec, the DSL grammar, and the graph schema.
pub const CURRENT_SCHEMA_VERSION: cofferdam_core::invariants::SchemaVersion =
    cofferdam_core::invariants::SchemaVersion::new(1, 0);

/// Oldest graph-schema version this build still accepts.
///
/// `>= MIN_SUPPORTED` and `< CURRENT` falls in the deprecation window
/// (accepted with a hint surfaced by [`GraphEnvelope::
/// schema_version_deprecated`]). Anything strictly below
/// `MIN_SUPPORTED` is rejected with a migration message.
pub const MIN_SUPPORTED_SCHEMA_VERSION: cofferdam_core::invariants::SchemaVersion =
    cofferdam_core::invariants::SchemaVersion::new(1, 0);
