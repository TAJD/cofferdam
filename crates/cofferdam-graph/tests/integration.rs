//! Integration tests for cofferdam-graph.
//!
//! Exercises the public API across multiple modules. Complements the
//! inline unit tests in store.rs / schema.rs / build.rs / loader.rs.
//! Only public re-exports from the crate root are used here.

use std::path::{Path, PathBuf};

use cofferdam_core::graph::{ImportKind, ImportRecord, ImportedName};
use cofferdam_core::Span;
use cofferdam_graph::{
    apply_records, build_canonical_graph, load_envelope, normalized_file_path, EdgeKind,
    EdgePayload, Graph, LoadError, NodeId, NodeKind,
};
use smol_str::SmolStr;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn span() -> Span {
    Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    }
}

fn file_node(path: &str) -> NodeKind {
    NodeKind::File {
        path: PathBuf::from(path),
        lang: SmolStr::new("typescript"),
    }
}

fn named_import(from: &str, to: &str, name: &str) -> ImportRecord {
    ImportRecord {
        from_file: PathBuf::from(from),
        source_specifier: "./m".to_string(),
        resolved: Some(PathBuf::from(to)),
        names: vec![ImportedName {
            source_name: name.to_string(),
            local_name: name.to_string(),
            kind: ImportKind::Named,
            type_only: false,
            local_use_count: 1,
        }],
        type_only: false,
        span: span(),
    }
}

fn export_record(path: &str, name: &str) -> cofferdam_core::graph::ExportRecord {
    cofferdam_core::graph::ExportRecord {
        file: PathBuf::from(path),
        name: name.to_string(),
        kind: cofferdam_core::graph::ExportKind::Named,
        type_only: false,
        span: span(),
        source_specifier: None,
        resolved_source: None,
    }
}

// ---------------------------------------------------------------------------
// Empty-graph edge cases
// ---------------------------------------------------------------------------

#[test]
fn empty_graph_has_zero_nodes_and_edges() {
    let g = Graph::new();
    assert_eq!(g.node_count(), 0);
    assert_eq!(g.edge_count(), 0);
}

#[test]
fn build_from_empty_records_is_empty() {
    let g = build_canonical_graph(&[], &[]);
    assert_eq!(g.node_count(), 0);
    assert_eq!(g.edge_count(), 0);
}

// ---------------------------------------------------------------------------
// Node/edge invariants — dangling endpoint panics
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "source node must be added before")]
fn add_edge_panics_for_missing_source() {
    let mut g = Graph::new();
    let dst = g.add_node(file_node("/p/b.ts"));
    // NodeId(0xdead_beef) was never inserted — add_edge must panic.
    g.add_edge(
        NodeId(0xdead_beef),
        dst,
        EdgePayload::bare(EdgeKind::ImportsAsValue),
    );
}

#[test]
#[should_panic(expected = "destination node must be added before")]
fn add_edge_panics_for_missing_destination() {
    let mut g = Graph::new();
    let src = g.add_node(file_node("/p/a.ts"));
    g.add_edge(
        src,
        NodeId(0xdead_beef),
        EdgePayload::bare(EdgeKind::ImportsAsValue),
    );
}

// ---------------------------------------------------------------------------
// Graph construction from records + query integration
// ---------------------------------------------------------------------------

#[test]
fn build_then_query_import_edge() {
    let a = "/proj/a.ts";
    let b = "/proj/b.ts";
    let g = build_canonical_graph(&[named_import(a, b, "Foo")], &[]);
    assert_eq!(g.node_count(), 2);
    assert_eq!(g.edge_count(), 1);

    let a_id = g
        .node_id_for_path(&normalized_file_path(&PathBuf::from(a)))
        .expect("a.ts node");
    let b_id = g
        .node_id_for_path(&normalized_file_path(&PathBuf::from(b)))
        .expect("b.ts node");

    let outgoing: Vec<_> = g.outgoing(a_id).collect();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, b_id);
    assert!(matches!(outgoing[0].1, EdgeKind::ImportsAsValue));

    assert_eq!(g.incoming(a_id).count(), 0);
    assert_eq!(g.incoming(b_id).count(), 1);
}

#[test]
fn pure_exporter_gets_isolated_node() {
    // A file that only exports and has nothing imported from it must
    // still get a File node so graph queries can observe it has no
    // incoming edges — that's the orphan-export signal.
    let exporter = "/proj/orphan.ts";
    let g = build_canonical_graph(&[], &[export_record(exporter, "Widget")]);
    assert_eq!(g.node_count(), 1);
    assert_eq!(g.edge_count(), 0);

    let id = g
        .node_id_for_path(&normalized_file_path(&PathBuf::from(exporter)))
        .expect("orphan exporter node");
    assert_eq!(g.incoming(id).count(), 0);
    assert_eq!(g.outgoing(id).count(), 0);
}

#[test]
fn multi_importer_graph_has_correct_edge_counts() {
    // Two files each importing a third module.
    let a = "/proj/a.ts";
    let b = "/proj/b.ts";
    let lib = "/proj/lib.ts";
    let g = build_canonical_graph(
        &[named_import(a, lib, "Foo"), named_import(b, lib, "Bar")],
        &[],
    );
    assert_eq!(g.node_count(), 3);
    assert_eq!(g.edge_count(), 2);

    let lib_id = g
        .node_id_for_path(&normalized_file_path(&PathBuf::from(lib)))
        .expect("lib node");
    // Two importers → two incoming edges.
    assert_eq!(g.incoming(lib_id).count(), 2);
}

// ---------------------------------------------------------------------------
// Edge case: self-referencing module
// ---------------------------------------------------------------------------

#[test]
fn self_referencing_module_has_one_node_and_one_self_edge() {
    // A module that imports from itself (unusual but a valid graph
    // state the builder must not silently drop). The File node is
    // deduped (from == to), but the edge is kept.
    let self_path = PathBuf::from("/proj/self_import.ts");
    let imp = ImportRecord {
        from_file: self_path.clone(),
        source_specifier: "./self_import".to_string(),
        resolved: Some(self_path.clone()),
        names: vec![ImportedName {
            source_name: "x".to_string(),
            local_name: "x".to_string(),
            kind: ImportKind::Named,
            type_only: false,
            local_use_count: 1,
        }],
        type_only: false,
        span: span(),
    };
    let g = build_canonical_graph(&[imp], &[]);
    assert_eq!(
        g.node_count(),
        1,
        "importer and target are the same file node"
    );
    assert_eq!(g.edge_count(), 1, "self-edge must not be dropped");

    let id = g
        .node_id_for_path(&normalized_file_path(&self_path))
        .expect("self-import node");
    let outgoing: Vec<_> = g.outgoing(id).collect();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, id, "self-edge target must be the same node");
}

// ---------------------------------------------------------------------------
// Schema / envelope serialization round-trip
// ---------------------------------------------------------------------------

#[test]
fn load_envelope_accepts_current_version_and_preserves_body() {
    let input = br#"{ "schema_version": "1.0", "nodes": [1, 2], "custom": true }"#;
    let env = load_envelope(input, None).expect("load should succeed");
    assert!(!env.schema_version_deprecated);
    // Non-version top-level keys arrive in body verbatim.
    assert!(env.body.contains_key("nodes"));
    assert!(env.body.contains_key("custom"));
    // The version key is consumed, not left in body.
    assert!(!env.body.contains_key("schema_version"));
}

#[test]
fn load_envelope_rejects_unknown_future_version() {
    let input = br#"{ "schema_version": "99.0" }"#;
    let err = load_envelope(input, None).unwrap_err();
    assert!(matches!(err, LoadError::FutureSchemaVersion { .. }));
    assert!(err.is_fatal());
}

// ---------------------------------------------------------------------------
// Per-file provenance + removal (cd-36)
// ---------------------------------------------------------------------------

/// Structural equality for tests: same node ids present, same edges
/// (as `(src, dst, payload)`), ignoring internal petgraph index /
/// insertion order. `NodeId` is a content hash so it's stable
/// regardless of how a node was reached.
fn assert_graphs_structurally_equal(a: &Graph, b: &Graph) {
    let mut a_nodes = a.node_ids();
    let mut b_nodes = b.node_ids();
    a_nodes.sort();
    b_nodes.sort();
    assert_eq!(a_nodes, b_nodes, "node sets differ");

    let mut a_edges: Vec<String> = a.all_edges().iter().map(|e| format!("{e:?}")).collect();
    let mut b_edges: Vec<String> = b.all_edges().iter().map(|e| format!("{e:?}")).collect();
    a_edges.sort();
    b_edges.sort();
    assert_eq!(a_edges, b_edges, "edge sets differ");
}

#[test]
fn remove_file_then_readd_matches_from_scratch_build() {
    let a = "/proj/a.ts";
    let b = "/proj/b.ts";
    let lib = "/proj/lib.ts";
    let imports = vec![named_import(a, lib, "Foo"), named_import(b, lib, "Bar")];
    let exports: Vec<cofferdam_core::graph::ExportRecord> = vec![];

    let fresh = build_canonical_graph(&imports, &exports);

    let mut rebuilt = build_canonical_graph(&imports, &exports);
    let a_path = normalized_file_path(&PathBuf::from(a));
    assert!(rebuilt.remove_file(&a_path), "a.ts owned something");

    // a.ts's own File node is gone, but lib.ts (shared import target)
    // must survive — b.ts still owns it.
    assert!(rebuilt.node_id_for_path(&a_path).is_none());
    let lib_id = rebuilt
        .node_id_for_path(&normalized_file_path(&PathBuf::from(lib)))
        .expect("lib.ts survives — b.ts still owns it");
    assert_eq!(rebuilt.incoming(lib_id).count(), 1, "only b.ts's edge left");

    // Re-add exactly a.ts's slice of evidence.
    let a_imports: Vec<_> = imports
        .iter()
        .filter(|i| i.from_file == Path::new(a))
        .cloned()
        .collect();
    apply_records(&mut rebuilt, &a_imports, &[]);

    assert_graphs_structurally_equal(&fresh, &rebuilt);
}

#[test]
fn remove_file_leaves_shared_import_target_intact() {
    let a = "/proj/a.ts";
    let b = "/proj/b.ts";
    let lib = "/proj/lib.ts";
    let mut g = build_canonical_graph(
        &[named_import(a, lib, "Foo"), named_import(b, lib, "Bar")],
        &[],
    );

    let a_path = normalized_file_path(&PathBuf::from(a));
    let lib_path = normalized_file_path(&PathBuf::from(lib));
    let lib_id_before = g.node_id_for_path(&lib_path).expect("lib node exists");

    g.remove_file(&a_path);

    assert!(
        g.node_id_for_path(&a_path).is_none(),
        "a.ts's own node is gone"
    );
    let lib_id_after = g
        .node_id_for_path(&lib_path)
        .expect("lib.ts node must survive — b.ts still owns it");
    assert_eq!(lib_id_before, lib_id_after);
    assert_eq!(g.incoming(lib_id_after).count(), 1, "b.ts's edge remains");
    assert_eq!(g.node_count(), 2, "b.ts + lib.ts remain");
    assert_eq!(g.edge_count(), 1);
}

#[test]
fn remove_file_is_noop_for_unowned_contributions() {
    // Nodes/edges added via the plain (unowned) API carry no
    // provenance; remove_file must not touch them.
    let mut g = Graph::new();
    let from = g.add_node(file_node("/p/a.ts"));
    let to = g.add_node(file_node("/p/b.ts"));
    g.add_edge(from, to, EdgePayload::bare(EdgeKind::ImportsAsValue));

    let changed = g.remove_file(&PathBuf::from("/p/a.ts"));
    assert!(!changed, "unowned contributions are untouched");
    assert_eq!(g.node_count(), 2);
    assert_eq!(g.edge_count(), 1);
}
