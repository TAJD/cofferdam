//! Petgraph-backed storage for the canonical graph.
//!
//! Builds on the schema types from [`crate::schema`] / [`crate::id`].
//! Wraps `petgraph::Graph<NodeKind, EdgePayload>` with two indexes the
//! cd-acv3 spike validated as the hot path:
//!
//! - **Path index** — `&Path → NodeId` for [`NodeKind::File`] nodes.
//!   The flat-table baseline rebuilds this lookup per query; pinning
//!   it once at build time is most of the closure-query speedup.
//! - **Layer index** — layer name → list of file `NodeId`s belonging
//!   to that layer. Mirrors the per-edge `BelongsToLayer` shape with
//!   a denormalised cache so `Design.LayerViolation`-style queries
//!   don't need to walk every file.
//!
//! Per the cd-9hp.9 ADR, edges carry both a `kind` (the closed-core
//! variant) and a sidecar `AttrMap` for per-edge metadata. The
//! sidecar is critical for the OrphanExport migration in cp3: a
//! single `import { foo, bar } from './m'` statement produces TWO
//! `ImportsAsValue` edges from the importing file to `./m`'s File
//! node, distinguished only by their `source_name` attribute. Keeping
//! attrs out of the [`EdgeKind`] enum (cp1's contract) lets the
//! schema match on shape exhaustively without exploding the variant
//! list.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use smol_str::SmolStr;

use crate::id::compute_node_id;
use crate::schema::{EdgeKind, NodeKind};
use crate::value::AttrMap;
use crate::NodeId;

/// Per-edge storage: the schema variant + a sidecar attribute map.
///
/// Two edges between the same `(src, dst)` pair with the same
/// [`EdgeKind`] discriminant are kept distinct when their `attrs`
/// differ — that's the shape `import { foo, bar } from './m'` needs
/// (two `ImportsAsValue` edges, one per binding name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgePayload {
    pub kind: EdgeKind,
    pub attrs: AttrMap,
}

impl EdgePayload {
    /// Convenience: no-attrs constructor. Useful for edges whose
    /// identity is fully captured by `(src, dst, kind)` — e.g.
    /// [`EdgeKind::BelongsToLayer`] or [`EdgeKind::DeclaredIn`].
    pub fn bare(kind: EdgeKind) -> Self {
        Self {
            kind,
            attrs: AttrMap::new(),
        }
    }
}

/// Canonical-graph storage. One per analysis run; populated by the
/// engine after pass-1 graph extraction, consumed by graph-aware
/// checks in finalize().
///
/// Build-only at cp2 — no removal or mutation API. Incremental
/// analysis (cd-9hp.4) will need a stable-index variant; we'll add
/// it under a separate type or behind a feature flag when cp4 lands.
#[derive(Debug, Default)]
pub struct Graph {
    inner: DiGraph<NodeKind, EdgePayload>,
    /// `NodeId → petgraph index`. Built incrementally as nodes are
    /// added. Required because the public API speaks `NodeId`
    /// (content-addressed, stable across processes) while petgraph
    /// internally uses `NodeIndex` (a generational handle).
    node_lookup: HashMap<NodeId, NodeIndex>,
    /// Path → NodeId for [`NodeKind::File`] nodes. The hot lookup
    /// validated by the cd-acv3 spike.
    path_index: HashMap<PathBuf, NodeId>,
    /// Layer name → file NodeIds belonging to that layer.
    layer_members: HashMap<SmolStr, Vec<NodeId>>,
}

impl Graph {
    /// Empty graph. Convention: callers build, then query — no
    /// removals at cp2.
    pub fn new() -> Self {
        Self::default()
    }

    // ----- mutation -----

    /// Add a node and return its [`NodeId`]. Idempotent: a node with
    /// the same identifying payload (per [`compute_node_id`]) is
    /// returned without duplication.
    ///
    /// Side effects on the indexes:
    /// - `NodeKind::File`: registers the path in `path_index`.
    /// - All others: no index touched at cp2.
    pub fn add_node(&mut self, node: NodeKind) -> NodeId {
        let id = compute_node_id(&node);
        if let Some(_existing) = self.node_lookup.get(&id) {
            return id;
        }
        // Register path-index entry BEFORE moving `node` into petgraph.
        if let NodeKind::File { path, .. } = &node {
            self.path_index.insert(path.clone(), id);
        }
        let idx = self.inner.add_node(node);
        self.node_lookup.insert(id, idx);
        id
    }

    /// Add a `BelongsToLayer` edge from a file node to a layer node,
    /// updating the layer-membership index. Returns the edge index
    /// (mostly useful for tests).
    ///
    /// The layer node is created lazily — callers don't need to
    /// `add_node` it separately.
    pub fn assign_layer(&mut self, file: NodeId, layer: &str) -> EdgeIndex {
        // Materialise the Layer node if it doesn't exist yet.
        let layer_id = self.add_node(NodeKind::Layer {
            name: SmolStr::new(layer),
        });
        self.layer_members
            .entry(SmolStr::new(layer))
            .or_default()
            .push(file);
        self.add_edge(file, layer_id, EdgePayload::bare(EdgeKind::BelongsToLayer))
    }

    /// Add an edge between two existing nodes. Panics if either
    /// endpoint isn't in the graph — callers must `add_node` first.
    ///
    /// Multiple edges between the same `(src, dst)` are allowed; the
    /// caller is responsible for distinguishing them via attrs.
    pub fn add_edge(&mut self, src: NodeId, dst: NodeId, payload: EdgePayload) -> EdgeIndex {
        let src_idx = self
            .node_lookup
            .get(&src)
            .copied()
            .expect("source node must be added before its edges");
        let dst_idx = self
            .node_lookup
            .get(&dst)
            .copied()
            .expect("destination node must be added before its edges");
        self.inner.add_edge(src_idx, dst_idx, payload)
    }

    // ----- size / introspection -----

    pub fn node_count(&self) -> usize {
        self.inner.node_count()
    }
    pub fn edge_count(&self) -> usize {
        self.inner.edge_count()
    }

    // ----- node lookup -----

    /// Look up a node by id. `None` when the id wasn't added.
    pub fn node(&self, id: NodeId) -> Option<&NodeKind> {
        let idx = *self.node_lookup.get(&id)?;
        Some(&self.inner[idx])
    }

    /// Path → file NodeId lookup. Returns `None` for paths that
    /// were never registered as `NodeKind::File`.
    pub fn node_id_for_path(&self, path: &Path) -> Option<NodeId> {
        self.path_index.get(path).copied()
    }

    /// File NodeIds that belong to the named layer. Empty slice when
    /// the layer wasn't declared OR no file was assigned to it.
    pub fn files_in_layer(&self, layer: &str) -> &[NodeId] {
        // Borrow trickery: `HashMap<SmolStr, _>` doesn't let us look
        // up by `&str` directly without allocating a SmolStr. We
        // accept the SmolStr allocation here because the layer index
        // is rarely hit on hot loops.
        self.layer_members
            .get(&SmolStr::new(layer))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    // ----- edge walks -----

    /// Outgoing edges from `node`, yielding
    /// `(target_id, &EdgeKind, &AttrMap)`. `None` iterator when the
    /// node isn't present.
    pub fn outgoing(
        &self,
        node: NodeId,
    ) -> Box<dyn Iterator<Item = (NodeId, &EdgeKind, &AttrMap)> + '_> {
        let Some(&src_idx) = self.node_lookup.get(&node) else {
            return Box::new(std::iter::empty());
        };
        Box::new(self.inner.edges(src_idx).map(move |e| {
            let target_idx = e.target();
            let target_id = self.id_for_index(target_idx);
            let payload = e.weight();
            (target_id, &payload.kind, &payload.attrs)
        }))
    }

    /// Incoming edges into `node`. Same yielding shape as
    /// [`Graph::outgoing`] — the first id is the SOURCE of each edge.
    pub fn incoming(
        &self,
        node: NodeId,
    ) -> Box<dyn Iterator<Item = (NodeId, &EdgeKind, &AttrMap)> + '_> {
        let Some(&dst_idx) = self.node_lookup.get(&node) else {
            return Box::new(std::iter::empty());
        };
        Box::new(
            self.inner
                .edges_directed(dst_idx, Direction::Incoming)
                .map(move |e| {
                    let source_idx = e.source();
                    let source_id = self.id_for_index(source_idx);
                    let payload = e.weight();
                    (source_id, &payload.kind, &payload.attrs)
                }),
        )
    }

    // ----- bounded BFS / closure -----

    /// Breadth-first reachable nodes from `start`, bounded to
    /// `max_depth` hops, following only edges whose `EdgeKind`
    /// satisfies `edge_filter`. The starting node is NOT included
    /// in the result.
    ///
    /// Yields ids in BFS order, deduplicated.
    pub fn bfs_outgoing(
        &self,
        start: NodeId,
        max_depth: usize,
        mut edge_filter: impl FnMut(&EdgeKind) -> bool,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();
        let Some(&start_idx) = self.node_lookup.get(&start) else {
            return out;
        };
        if max_depth == 0 {
            return out;
        }
        let mut visited: HashSet<NodeIndex> = HashSet::new();
        visited.insert(start_idx);
        let mut frontier: VecDeque<(NodeIndex, usize)> = VecDeque::new();
        frontier.push_back((start_idx, 0));
        while let Some((node_idx, depth)) = frontier.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for edge in self.inner.edges(node_idx) {
                if !edge_filter(&edge.weight().kind) {
                    continue;
                }
                let tgt_idx = edge.target();
                if !visited.insert(tgt_idx) {
                    continue;
                }
                out.push(self.id_for_index(tgt_idx));
                frontier.push_back((tgt_idx, depth + 1));
            }
        }
        out
    }

    /// Does `from` transitively reach `target` via edges satisfying
    /// `edge_filter`, within `max_depth` hops? Convenience wrapper
    /// around [`Graph::bfs_outgoing`] for the common "is X reachable
    /// from Y" predicate.
    pub fn reaches_via(
        &self,
        from: NodeId,
        target: NodeId,
        max_depth: usize,
        edge_filter: impl FnMut(&EdgeKind) -> bool,
    ) -> bool {
        if from == target {
            return true;
        }
        self.bfs_outgoing(from, max_depth, edge_filter)
            .into_iter()
            .any(|n| n == target)
    }

    // ----- internal helpers -----

    /// Reverse-lookup: petgraph `NodeIndex → NodeId`. Linear scan
    /// over `node_lookup`. Hot enough during BFS that a future
    /// optimisation may want to maintain the reverse map; for cp2 the
    /// O(N) hit is acceptable — measured ≤ 1µs per call on the spike's
    /// largest fixture.
    fn id_for_index(&self, idx: NodeIndex) -> NodeId {
        // Defensive: if the petgraph node is in the graph, it MUST
        // be in node_lookup (we only ever insert via add_node which
        // populates both). A failure here is an invariant break.
        for (id, registered) in &self.node_lookup {
            if *registered == idx {
                return *id;
            }
        }
        panic!("petgraph NodeIndex {idx:?} missing from node_lookup — invariant violation");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::schema::ExportKind;
    use crate::value::Value;

    fn file(path: &str) -> NodeKind {
        NodeKind::File {
            path: PathBuf::from(path),
            lang: SmolStr::new("typescript"),
        }
    }

    fn export(name: &str) -> NodeKind {
        NodeKind::Export {
            name: SmolStr::new(name),
        }
    }

    // -----------------------------------------------------------------------
    // add_node dedupe
    // -----------------------------------------------------------------------

    #[test]
    fn add_node_dedupes_by_identity() {
        let mut g = Graph::new();
        let a = g.add_node(file("/proj/foo.ts"));
        let b = g.add_node(file("/proj/foo.ts"));
        assert_eq!(a, b, "second add of same payload must return same id");
        assert_eq!(g.node_count(), 1, "petgraph must not have duplicated");
    }

    #[test]
    fn add_node_distinguishes_different_paths() {
        let mut g = Graph::new();
        let a = g.add_node(file("/proj/foo.ts"));
        let b = g.add_node(file("/proj/bar.ts"));
        assert_ne!(a, b);
        assert_eq!(g.node_count(), 2);
    }

    // -----------------------------------------------------------------------
    // path index
    // -----------------------------------------------------------------------

    #[test]
    fn path_index_populated_on_file_add() {
        let mut g = Graph::new();
        let id = g.add_node(file("/proj/foo.ts"));
        assert_eq!(g.node_id_for_path(Path::new("/proj/foo.ts")), Some(id));
        assert_eq!(g.node_id_for_path(Path::new("/proj/missing.ts")), None);
    }

    #[test]
    fn path_index_only_tracks_files() {
        let mut g = Graph::new();
        g.add_node(export("Widget"));
        // Export nodes do not enter the path index.
        assert_eq!(g.node_id_for_path(Path::new("Widget")), None);
    }

    // -----------------------------------------------------------------------
    // layer index
    // -----------------------------------------------------------------------

    #[test]
    fn assign_layer_creates_node_and_edge() {
        let mut g = Graph::new();
        let f = g.add_node(file("/proj/app/page.ts"));
        g.assign_layer(f, "app");
        assert_eq!(g.files_in_layer("app"), &[f]);
        assert_eq!(g.files_in_layer("infra"), &[] as &[NodeId]);
        // Layer node was materialised.
        assert!(g.node_count() == 2, "file + layer node");
        assert!(g.edge_count() == 1, "single BelongsToLayer edge");
    }

    #[test]
    fn assign_layer_multiple_files() {
        let mut g = Graph::new();
        let a = g.add_node(file("/proj/app/a.ts"));
        let b = g.add_node(file("/proj/app/b.ts"));
        g.assign_layer(a, "app");
        g.assign_layer(b, "app");
        let members = g.files_in_layer("app");
        assert_eq!(members.len(), 2);
        assert!(members.contains(&a) && members.contains(&b));
    }

    // -----------------------------------------------------------------------
    // edges
    // -----------------------------------------------------------------------

    fn imports_value_attrs(source_name: &str) -> AttrMap {
        let mut m = AttrMap::new();
        m.insert(SmolStr::new("source_name"), Value::from(source_name));
        m
    }

    #[test]
    fn add_edge_preserves_distinct_attrs() {
        // Models `import { foo, bar } from './m'` — two value-imports
        // between the same file pair, distinguished by source_name.
        let mut g = Graph::new();
        let from = g.add_node(file("/proj/a.ts"));
        let to = g.add_node(file("/proj/m.ts"));
        g.add_edge(
            from,
            to,
            EdgePayload {
                kind: EdgeKind::ImportsAsValue,
                attrs: imports_value_attrs("foo"),
            },
        );
        g.add_edge(
            from,
            to,
            EdgePayload {
                kind: EdgeKind::ImportsAsValue,
                attrs: imports_value_attrs("bar"),
            },
        );
        assert_eq!(g.edge_count(), 2, "distinct attrs → distinct edges");
        let names: Vec<&str> = g
            .outgoing(from)
            .map(
                |(_, _, attrs)| match attrs.get(&SmolStr::new("source_name")) {
                    Some(Value::String(s)) => s.as_str(),
                    _ => "?",
                },
            )
            .collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn outgoing_yields_kind_and_attrs() {
        let mut g = Graph::new();
        let from = g.add_node(file("/proj/a.ts"));
        let to = g.add_node(file("/proj/b.ts"));
        g.add_edge(
            from,
            to,
            EdgePayload::bare(EdgeKind::ExportsAs {
                kind: ExportKind::Named,
            }),
        );
        let out: Vec<_> = g.outgoing(from).collect();
        assert_eq!(out.len(), 1);
        let (target, kind, _attrs) = &out[0];
        assert_eq!(*target, to);
        assert!(matches!(
            kind,
            EdgeKind::ExportsAs {
                kind: ExportKind::Named
            }
        ));
    }

    #[test]
    fn incoming_mirrors_outgoing() {
        let mut g = Graph::new();
        let from = g.add_node(file("/proj/a.ts"));
        let to = g.add_node(file("/proj/b.ts"));
        g.add_edge(from, to, EdgePayload::bare(EdgeKind::ImportsAsValue));
        let inc: Vec<_> = g.incoming(to).map(|(src, _, _)| src).collect();
        assert_eq!(inc, vec![from]);
        // And the source has no incoming.
        assert_eq!(g.incoming(from).count(), 0);
    }

    #[test]
    fn outgoing_for_missing_node_is_empty() {
        let g = Graph::new();
        let count = g.outgoing(NodeId(0xdead_beef)).count();
        assert_eq!(count, 0);
    }

    // -----------------------------------------------------------------------
    // BFS / closure
    // -----------------------------------------------------------------------

    fn chain(g: &mut Graph, files: &[&str]) -> Vec<NodeId> {
        let ids: Vec<NodeId> = files.iter().map(|p| g.add_node(file(p))).collect();
        for w in ids.windows(2) {
            g.add_edge(w[0], w[1], EdgePayload::bare(EdgeKind::ImportsAsValue));
        }
        ids
    }

    #[test]
    fn bfs_respects_depth_bound() {
        let mut g = Graph::new();
        let ids = chain(&mut g, &["/p/a.ts", "/p/b.ts", "/p/c.ts", "/p/d.ts"]);
        // depth 1: only b.
        let r1 = g.bfs_outgoing(ids[0], 1, |_| true);
        assert_eq!(r1, vec![ids[1]]);
        // depth 2: b, c.
        let r2 = g.bfs_outgoing(ids[0], 2, |_| true);
        assert_eq!(r2, vec![ids[1], ids[2]]);
        // depth 3: b, c, d.
        let r3 = g.bfs_outgoing(ids[0], 3, |_| true);
        assert_eq!(r3, vec![ids[1], ids[2], ids[3]]);
        // depth 0: nothing.
        let r0 = g.bfs_outgoing(ids[0], 0, |_| true);
        assert!(r0.is_empty());
    }

    #[test]
    fn bfs_respects_edge_filter() {
        let mut g = Graph::new();
        let a = g.add_node(file("/p/a.ts"));
        let b = g.add_node(file("/p/b.ts"));
        let c = g.add_node(file("/p/c.ts"));
        // a --(value)--> b --(type)--> c
        g.add_edge(a, b, EdgePayload::bare(EdgeKind::ImportsAsValue));
        g.add_edge(b, c, EdgePayload::bare(EdgeKind::ImportsAsType));
        // Filter to value-only — stops at b.
        let value_only = g.bfs_outgoing(a, 8, |k| matches!(k, EdgeKind::ImportsAsValue));
        assert_eq!(value_only, vec![b]);
        // No filter — reaches c.
        let all = g.bfs_outgoing(a, 8, |_| true);
        assert_eq!(all, vec![b, c]);
    }

    #[test]
    fn bfs_dedupes_diamond() {
        // a → b → d
        //  ↘ c ↗
        let mut g = Graph::new();
        let a = g.add_node(file("/p/a.ts"));
        let b = g.add_node(file("/p/b.ts"));
        let c = g.add_node(file("/p/c.ts"));
        let d = g.add_node(file("/p/d.ts"));
        for (src, dst) in [(a, b), (a, c), (b, d), (c, d)] {
            g.add_edge(src, dst, EdgePayload::bare(EdgeKind::ImportsAsValue));
        }
        let out = g.bfs_outgoing(a, 8, |_| true);
        // d appears once even though two paths reach it.
        assert_eq!(out.iter().filter(|n| **n == d).count(), 1);
    }

    // -----------------------------------------------------------------------
    // reaches_via
    // -----------------------------------------------------------------------

    #[test]
    fn reaches_via_finds_target() {
        let mut g = Graph::new();
        let ids = chain(&mut g, &["/p/a.ts", "/p/b.ts", "/p/c.ts"]);
        assert!(g.reaches_via(ids[0], ids[2], 8, |_| true));
        assert!(
            !g.reaches_via(ids[2], ids[0], 8, |_| true),
            "edges are directed"
        );
    }

    #[test]
    fn reaches_via_respects_depth() {
        let mut g = Graph::new();
        let ids = chain(&mut g, &["/p/a.ts", "/p/b.ts", "/p/c.ts", "/p/d.ts"]);
        assert!(!g.reaches_via(ids[0], ids[3], 2, |_| true));
        assert!(g.reaches_via(ids[0], ids[3], 3, |_| true));
    }

    #[test]
    fn reaches_via_self_is_true() {
        let mut g = Graph::new();
        let a = g.add_node(file("/p/a.ts"));
        assert!(g.reaches_via(a, a, 0, |_| true));
    }
}
