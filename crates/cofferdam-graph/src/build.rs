//! Builders that translate flat-table evidence into a canonical
//! [`Graph`].
//!
//! cd-9hp.9 cp3 — `Design.OrphanExport`'s migration consumes a
//! pre-built `Graph` rather than joining `IMPORTS` / `EXPORTS` corpus
//! slots itself. The engine calls [`build_canonical_graph`] once after
//! pass 1 finishes and writes the result into the
//! [`crate::corpus_slot::CANONICAL_GRAPH`] slot; OrphanExport reads
//! that slot in `finalize`. Per the bead's backward-compat path, the
//! flat-table slots stay populated for the other graph-aware checks
//! until they migrate.
//!
//! ## What gets materialised
//!
//! - One [`NodeKind::File`] node per distinct (normalised) absolute
//!   path mentioned in any record — both the importing file and any
//!   resolved import target. Bare specifiers that didn't resolve
//!   (`react`, `lodash`) DON'T get File nodes; they leave the project
//!   graph by design.
//! - One import edge per [`cofferdam_core::graph::ImportedName`] — or
//!   one bare edge for a side-effect import that carries no names.
//!   The edge variant is [`EdgeKind::ImportsAsType`] when the import
//!   (or per-name) is marked type-only, otherwise
//!   [`EdgeKind::ImportsAsValue`]. Edge attributes:
//!     - `source_name` — the export name in the target module
//!       (`"default"` for default imports, `"*"` for namespace).
//!     - `local_name` — the binding name in the importing file.
//!     - `import_kind` — one of `"default" | "named" | "namespace"
//!       | "side_effect"`.
//!     - `type_only` — bool, mirrors the per-edge type-only flag so
//!       graph consumers don't have to recompute it from the variant.
//!
//! Export nodes and [`EdgeKind::ExportsAs`] edges are intentionally
//! NOT materialised at cp3. OrphanExport's query path walks incoming
//! import edges on each file node — adding export nodes with no
//! consumer is dead weight until `Design.DeadExport`,
//! `Design.ImportCycle`, and `Design.LayerViolation` migrate too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{ExportRecord, ImportKind, ImportRecord};
use smol_str::SmolStr;

use crate::schema::{EdgeKind, NodeKind};
use crate::store::{EdgePayload, Graph};
use crate::value::{AttrMap, Value};

/// Normalise a filesystem path for canonical-graph identity.
///
/// Cofferdam's flat-table layer already collapses `C:\Foo` and
/// `C:\foo` via the private `path_key()` helper in
/// `cofferdam-checks::design`; the canonical graph keys on the same
/// shape so engine writes and check reads always look up the same
/// `NodeId` even when the resolver and the discovery pass disagree on
/// case.
///
/// Semantics:
/// - Backslashes → forward slashes (matches the `hash_path`
///   normaliser used by [`crate::id::compute_node_id`]).
/// - Lowercased on Windows hosts (the filesystem is case-insensitive
///   there).
/// - Other platforms preserve case.
pub fn normalized_file_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        PathBuf::from(s.to_lowercase())
    } else {
        PathBuf::from(s)
    }
}

/// Build the canonical graph from per-file import/export evidence.
///
/// See the module docs for the materialisation contract. The
/// returned graph is owned by the caller; cofferdam-engine writes it
/// into the [`crate::corpus_slot::CANONICAL_GRAPH`] slot for
/// finalize-stage checks to query.
pub fn build_canonical_graph(imports: &[ImportRecord], exports: &[ExportRecord]) -> Graph {
    let mut g = Graph::new();

    // First pass: materialise every File node we'll need so each
    // `add_edge` call below can assume both endpoints are present.
    // Files appear here as either importer (`imp.from_file`), import
    // target (`imp.resolved`), or pure exporter (`exp.file`) — the
    // last case is what makes orphan exports queryable at all: a
    // file that only exports and has nothing imported from it would
    // otherwise have no node.
    let mut paths: HashSet<PathBuf> = HashSet::new();
    for imp in imports {
        paths.insert(normalized_file_path(&imp.from_file));
        if let Some(r) = &imp.resolved {
            paths.insert(normalized_file_path(r));
        }
    }
    for exp in exports {
        paths.insert(normalized_file_path(&exp.file));
    }
    for p in paths {
        g.add_node(NodeKind::File {
            path: p,
            lang: SmolStr::new_static("typescript"),
        });
    }

    for imp in imports {
        let Some(resolved) = &imp.resolved else {
            continue;
        };
        let from = normalized_file_path(&imp.from_file);
        let to = normalized_file_path(resolved);
        let (Some(from_id), Some(to_id)) = (g.node_id_for_path(&from), g.node_id_for_path(&to))
        else {
            continue;
        };

        if imp.names.is_empty() {
            // `import './polyfill'` — no consumption claim against any
            // export name. Edge recorded so future graph queries can
            // see that the module was touched at all; OrphanExport
            // ignores `side_effect` edges in its consumption logic.
            let mut attrs = AttrMap::new();
            attrs.insert(
                SmolStr::new_static("import_kind"),
                Value::from("side_effect"),
            );
            attrs.insert(SmolStr::new_static("type_only"), Value::from(imp.type_only));
            let kind = if imp.type_only {
                EdgeKind::ImportsAsType
            } else {
                EdgeKind::ImportsAsValue
            };
            g.add_edge(from_id, to_id, EdgePayload { kind, attrs });
            continue;
        }

        for n in &imp.names {
            let type_only = imp.type_only || n.type_only;
            let mut attrs = AttrMap::new();
            attrs.insert(
                SmolStr::new_static("source_name"),
                Value::from(n.source_name.as_str()),
            );
            attrs.insert(
                SmolStr::new_static("local_name"),
                Value::from(n.local_name.as_str()),
            );
            let import_kind = match n.kind {
                ImportKind::Default => "default",
                ImportKind::Named => "named",
                ImportKind::Namespace => "namespace",
            };
            attrs.insert(SmolStr::new_static("import_kind"), Value::from(import_kind));
            attrs.insert(SmolStr::new_static("type_only"), Value::from(type_only));
            let kind = if type_only {
                EdgeKind::ImportsAsType
            } else {
                EdgeKind::ImportsAsValue
            };
            g.add_edge(from_id, to_id, EdgePayload { kind, attrs });
        }
    }

    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ExportKind, ExportRecord, ImportedName};
    use cofferdam_core::Span;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        }
    }

    fn named_import(from: &Path, to: &Path, source_name: &str, local_name: &str) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./m".to_string(),
            resolved: Some(to.to_path_buf()),
            names: vec![ImportedName {
                source_name: source_name.to_string(),
                local_name: local_name.to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    fn named_export(file: &Path, name: &str) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::Named,
            type_only: false,
            span: span(),
            source_specifier: None,
            resolved_source: None,
        }
    }

    #[test]
    fn materialises_one_file_node_per_distinct_path() {
        let a = PathBuf::from("/proj/a.ts");
        let b = PathBuf::from("/proj/b.ts");
        let imports = vec![named_import(&a, &b, "X", "X")];
        let exports = vec![named_export(&b, "X")];
        let g = build_canonical_graph(&imports, &exports);
        // Two files, one edge.
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert!(g.node_id_for_path(&normalized_file_path(&a)).is_some());
        assert!(g.node_id_for_path(&normalized_file_path(&b)).is_some());
    }

    #[test]
    fn materialises_file_node_for_pure_exporter() {
        // A file that only exports (no imports targeting it from
        // anywhere) must still get a File node so OrphanExport can
        // walk its incoming edges (which will be empty — that's the
        // orphan signal).
        let exporter = PathBuf::from("/proj/only_exports.ts");
        let exports = vec![named_export(&exporter, "Forgotten")];
        let g = build_canonical_graph(&[], &exports);
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 0);
        assert!(g
            .node_id_for_path(&normalized_file_path(&exporter))
            .is_some());
    }

    #[test]
    fn skips_imports_with_unresolved_target() {
        let a = PathBuf::from("/proj/a.ts");
        // Bare specifier — `resolved: None` means it leaves the
        // project graph (`react`, `lodash` …). The from_file gets a
        // node but no edge is laid down.
        let imp = ImportRecord {
            from_file: a.clone(),
            source_specifier: "react".to_string(),
            resolved: None,
            names: vec![ImportedName {
                source_name: "default".to_string(),
                local_name: "React".to_string(),
                kind: ImportKind::Default,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        };
        let g = build_canonical_graph(&[imp], &[]);
        assert_eq!(g.node_count(), 1);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn one_edge_per_imported_name() {
        // `import { foo, bar } from './m'` lands as ONE ImportRecord
        // with two names — two edges in the graph.
        let a = PathBuf::from("/proj/a.ts");
        let m = PathBuf::from("/proj/m.ts");
        let imp = ImportRecord {
            from_file: a.clone(),
            source_specifier: "./m".to_string(),
            resolved: Some(m.clone()),
            names: vec![
                ImportedName {
                    source_name: "foo".to_string(),
                    local_name: "foo".to_string(),
                    kind: ImportKind::Named,
                    type_only: false,
                    local_use_count: 1,
                },
                ImportedName {
                    source_name: "bar".to_string(),
                    local_name: "bar".to_string(),
                    kind: ImportKind::Named,
                    type_only: false,
                    local_use_count: 1,
                },
            ],
            type_only: false,
            span: span(),
        };
        let g = build_canonical_graph(&[imp], &[]);
        assert_eq!(g.edge_count(), 2);
    }

    #[test]
    fn type_only_imports_use_imports_as_type_variant() {
        let a = PathBuf::from("/proj/a.ts");
        let m = PathBuf::from("/proj/m.ts");
        let imp = ImportRecord {
            from_file: a,
            source_specifier: "./m".to_string(),
            resolved: Some(m.clone()),
            names: vec![ImportedName {
                source_name: "T".to_string(),
                local_name: "T".to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: true, // statement-level `import type { T } …`
            span: span(),
        };
        let g = build_canonical_graph(&[imp], &[]);
        let to_id = g
            .node_id_for_path(&normalized_file_path(&m))
            .expect("target node");
        let edges: Vec<_> = g.incoming(to_id).collect();
        assert_eq!(edges.len(), 1);
        assert!(matches!(edges[0].1, EdgeKind::ImportsAsType));
    }

    #[test]
    fn side_effect_import_records_bare_edge() {
        let a = PathBuf::from("/proj/a.ts");
        let polyfill = PathBuf::from("/proj/polyfill.ts");
        let imp = ImportRecord {
            from_file: a,
            source_specifier: "./polyfill".to_string(),
            resolved: Some(polyfill.clone()),
            names: vec![], // side-effect
            type_only: false,
            span: span(),
        };
        let g = build_canonical_graph(&[imp], &[]);
        let to_id = g
            .node_id_for_path(&normalized_file_path(&polyfill))
            .expect("polyfill node");
        let edges: Vec<_> = g.incoming(to_id).collect();
        assert_eq!(edges.len(), 1);
        let (_src, _kind, attrs) = &edges[0];
        let import_kind = attrs.get(&SmolStr::new_static("import_kind"));
        assert_eq!(
            import_kind,
            Some(&Value::String(SmolStr::new_static("side_effect")))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_insensitive_paths_dedupe() {
        // C:\Foo\m.ts and C:\foo\m.ts must land on the same File
        // node — matches the path_key()-style normalisation that the
        // flat-table layer already does.
        let a = PathBuf::from(r"C:\proj\A.ts");
        let m_upper = PathBuf::from(r"C:\Proj\M.ts");
        let m_lower = PathBuf::from(r"c:\proj\m.ts");
        let imports = vec![
            named_import(&a, &m_upper, "X", "X"),
            named_import(&a, &m_lower, "Y", "Y"),
        ];
        let g = build_canonical_graph(&imports, &[]);
        // a.ts + the (single) target — two nodes total.
        assert_eq!(g.node_count(), 2);
        // Two edges, both into the same target.
        let target_id = g
            .node_id_for_path(&normalized_file_path(&m_lower))
            .expect("normalised target id");
        let incoming: Vec<_> = g.incoming(target_id).collect();
        assert_eq!(incoming.len(), 2);
    }
}
