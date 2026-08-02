//! `Context.BlastRadius` (CD-160) — bounded-depth BFS over the
//! canonical import graph from changed files, surfacing:
//!
//! - direct importers of a changed file,
//! - direct callers of a changed exported symbol (an incoming import
//!   edge whose `source_name` matches an export whose span overlaps
//!   the diff),
//! - transitive importers reached within a bounded depth, and
//! - test files that reach the change (any of the above, annotated).
//!
//! Reuses the `CANONICAL_GRAPH` corpus slot the same way
//! `Design.OrphanExport` does — see that check for the query-path
//! precedent. The canonical graph only materialises File→File import
//! edges (no function-level call graph), so "callers of a changed
//! exported symbol" is approximated at file granularity: a file that
//! imports the specific changed binding by name, not a file that
//! calls the corresponding function at a particular call site.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ExportKind as GExportKind, ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS,
    IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, FinalizeContext, Issue,
    Location, RelatedSpan, Severity, SourceFile, Span,
};
use cofferdam_graph::{normalized_file_path, EdgeKind, Graph, NodeId, Value, CANONICAL_GRAPH};
use smol_str::SmolStr;

/// Filename substrings marking a file as a test file. Mirrors
/// `Design.OrphanExport`/`Design.MissingTestFile`'s default list; kept
/// as a private copy (no options in v1) rather than shared — those two
/// checks already duplicate the list for the same reason (see
/// `design/missing_test_file.rs`'s comment on why the lists aren't
/// pulled into one shared constant).
const TEST_FILE_PATTERNS: &[&str] = &[
    ".test.",
    ".spec.",
    "_test.",
    "_spec.",
    "/__tests__/",
    "/__mocks__/",
];

/// Bounded BFS depth (hops through the incoming-import graph). No
/// tunable yet (v1 ships no options) — 4 hops is generous enough to
/// surface indirect blast radius without the traversal turning into a
/// project-wide reachability dump on a large repo.
const MAX_DEPTH: usize = 4;

const META: CheckMeta = CheckMeta {
    id: "Context.BlastRadius",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "Importers, callers of changed exported symbols, and test files that reach a changed file, via a bounded-depth walk of the canonical import graph.",
    body: include_str!("../../docs/Context.BlastRadius.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

/// `Context.BlastRadius` — advisory provider for `cofferdam context`
/// (CD-160). Emits nothing under `cofferdam check` (empty `run`,
/// `Category::Context` is never dispatched there).
pub struct BlastRadius;

impl Check for BlastRadius {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn context_items(
        &self,
        changeset: &ChangeSet,
        ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        ctx.corpus.with_slot(&CANONICAL_GRAPH, |graph| {
            compute_blast_radius(graph, &exports, &imports, changeset)
        })
    }
}

/// Zero-span placeholder for reporting a whole-file relation where no
/// specific declaration site is being pointed at. Mirrors the same
/// fallback pattern `Design.ImportCycle` uses when a cycle-member's
/// import span can't be resolved.
fn zero_span() -> Span {
    Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    }
}

fn is_test_path(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    TEST_FILE_PATTERNS.iter().any(|p| normalized.contains(p))
}

/// Per-reached-node BFS result: shortest depth from any changed file,
/// whether the depth-1 edge that discovered it named a changed export
/// directly, and the file-path chain from the changed root (index 0)
/// out to this node (last).
struct ReachInfo {
    depth: usize,
    symbol_match: bool,
    chain: Vec<PathBuf>,
}

/// Original-case display path for every path the flat-table evidence
/// mentions, keyed by the same normalised identity the canonical graph
/// uses. The graph itself only stores the lowercased/forward-slashed
/// form (Windows case-insensitivity, see `normalized_file_path`), so
/// display text is recovered from `IMPORTS`/`EXPORTS` instead.
fn display_paths(imports: &[ImportRecord], exports: &[ExportRecord]) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    for imp in imports {
        map.entry(path_key(&imp.from_file))
            .or_insert_with(|| imp.from_file.clone());
        if let Some(resolved) = &imp.resolved {
            map.entry(path_key(resolved))
                .or_insert_with(|| resolved.clone());
        }
    }
    for exp in exports {
        map.entry(path_key(&exp.file))
            .or_insert_with(|| exp.file.clone());
    }
    map
}

/// Names of real (non-type-only, non-re-export) exports declared in
/// `file` whose span overlaps `changed`'s line ranges for that file.
/// `None` ranges (whole-file mode, per `ChangeSet` doc) includes every
/// real export.
fn changed_export_names(
    exports: &[ExportRecord],
    file: &Path,
    changed: &ChangeSet,
) -> HashSet<SmolStr> {
    let file_key = path_key(file);
    let ranges = changed.line_ranges.get(file);
    exports
        .iter()
        .filter(|e| path_key(&e.file) == file_key)
        .filter(|e| !e.type_only && !matches!(e.kind, GExportKind::ReExport))
        .filter(|e| match ranges {
            None => true,
            Some(rs) if rs.is_empty() => true,
            Some(rs) => rs
                .iter()
                .any(|r| e.span.line >= r.start && e.span.line <= r.end),
        })
        .map(|e| {
            if matches!(e.kind, GExportKind::Default) {
                SmolStr::new_static("default")
            } else {
                SmolStr::new(&e.name)
            }
        })
        .collect()
}

fn source_name_of(attrs: &cofferdam_graph::AttrMap) -> Option<SmolStr> {
    match attrs.get(&SmolStr::new_static("source_name")) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Bounded BFS over incoming import edges starting at `root`. Returns
/// every reached node's shortest-depth [`ReachInfo`]. Deterministic:
/// at each level, candidate edges are sorted by source file path
/// before being folded into `result`/the frontier, so iteration order
/// never depends on petgraph's internal (insertion-order-derived)
/// edge ordering.
fn bfs_incoming(
    g: &Graph,
    root: NodeId,
    root_path: &Path,
    changed_names: &HashSet<SmolStr>,
    display: &HashMap<String, PathBuf>,
) -> HashMap<NodeId, ReachInfo> {
    let mut result: HashMap<NodeId, ReachInfo> = HashMap::new();
    let mut visited: HashSet<NodeId> = HashSet::new();
    visited.insert(root);
    let mut frontier: VecDeque<(NodeId, usize, Vec<PathBuf>)> = VecDeque::new();
    frontier.push_back((root, 0, vec![root_path.to_path_buf()]));

    while let Some((node, depth, chain)) = frontier.pop_front() {
        if depth >= MAX_DEPTH {
            continue;
        }
        let mut candidates: Vec<(NodeId, PathBuf, bool)> = g
            .incoming(node)
            .filter(|(_, kind, _)| {
                matches!(kind, EdgeKind::ImportsAsValue | EdgeKind::ImportsAsType)
            })
            .filter_map(|(src, _kind, attrs)| {
                let src_kind = g.node(src)?;
                let cofferdam_graph::NodeKind::File { path, .. } = src_kind else {
                    return None;
                };
                let display_path = display
                    .get(&path_key(path))
                    .cloned()
                    .unwrap_or_else(|| path.clone());
                let symbol_match =
                    depth == 0 && source_name_of(attrs).is_some_and(|n| changed_names.contains(&n));
                Some((src, display_path, symbol_match))
            })
            .collect();
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));

        for (src, src_path, symbol_match) in candidates {
            let mut new_chain = chain.clone();
            new_chain.push(src_path);
            let new_depth = depth + 1;

            let should_write = match result.get(&src) {
                None => true,
                Some(existing) => {
                    new_depth < existing.depth
                        || (new_depth == existing.depth && symbol_match && !existing.symbol_match)
                }
            };
            if should_write {
                result.insert(
                    src,
                    ReachInfo {
                        depth: new_depth,
                        symbol_match,
                        chain: new_chain.clone(),
                    },
                );
            }
            if visited.insert(src) {
                frontier.push_back((src, new_depth, new_chain));
            }
        }
    }

    result
}

fn relation_label(depth: usize, symbol_match: bool) -> String {
    if symbol_match {
        "calls a changed export directly".to_string()
    } else if depth == 1 {
        "imports the changed file directly".to_string()
    } else {
        format!("reaches the change transitively ({depth} hops)")
    }
}

fn score_for(depth: usize, symbol_match: bool, is_test: bool) -> i32 {
    let base = if symbol_match { 100 } else { 70 };
    let decay = i32::try_from(depth.saturating_sub(1)).unwrap_or(0) * 15;
    let test_bonus = if is_test { 5 } else { 0 };
    (base - decay + test_bonus).max(5)
}

fn explain_for(chain: &[PathBuf]) -> String {
    // `chain` runs [changed_root, depth1, depth2, ..., reached]. The
    // human-readable path reads outside-in: reached imports ... imports
    // changed root.
    let mut segments: Vec<String> = chain
        .iter()
        .rev()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned())
        })
        .collect();
    if let Some(last) = segments.last_mut() {
        *last = format!("changed {last}");
    }
    segments.join(" imports ")
}

fn compute_blast_radius(
    g: &Graph,
    exports: &[ExportRecord],
    imports: &[ImportRecord],
    changeset: &ChangeSet,
) -> Vec<ContextItem> {
    if changeset.is_empty() {
        return Vec::new();
    }
    let display = display_paths(imports, exports);
    let changed_node_ids: HashSet<NodeId> = changeset
        .files
        .iter()
        .filter_map(|f| g.node_id_for_path(&normalized_file_path(f)))
        .collect();

    let mut merged: HashMap<NodeId, ReachInfo> = HashMap::new();
    for changed_file in &changeset.files {
        let normalized = normalized_file_path(changed_file);
        let Some(root_id) = g.node_id_for_path(&normalized) else {
            continue;
        };
        let changed_names = changed_export_names(exports, changed_file, changeset);
        let reached = bfs_incoming(g, root_id, changed_file, &changed_names, &display);
        for (node_id, info) in reached {
            if changed_node_ids.contains(&node_id) {
                continue;
            }
            let better = match merged.get(&node_id) {
                None => true,
                Some(existing) => {
                    info.depth < existing.depth
                        || (info.depth == existing.depth
                            && info.symbol_match
                            && !existing.symbol_match)
                        || (info.depth == existing.depth
                            && info.symbol_match == existing.symbol_match
                            && info.chain.last() < existing.chain.last())
                }
            };
            if better {
                merged.insert(node_id, info);
            }
        }
    }

    let mut items: Vec<ContextItem> = merged
        .into_values()
        .filter_map(|info| {
            let path = info.chain.last()?.clone();
            let is_test = is_test_path(&path);
            let label = relation_label(info.depth, info.symbol_match);
            let score = score_for(info.depth, info.symbol_match, is_test);
            let explain = explain_for(&info.chain);
            let test_note = if is_test { " (test file)" } else { "" };
            let title = format!("Blast radius: {}", path.display());
            let body = format!("`{}` {label}{test_note}.", path.display());
            let changed_root = info.chain.first()?.clone();
            Some(ContextItem {
                check_id: META.id.to_string(),
                title,
                body,
                score,
                pinned: false,
                related: vec![
                    RelatedSpan {
                        location: Location::from_span(&path, zero_span()),
                        file: path,
                    },
                    RelatedSpan {
                        location: Location::from_span(&changed_root, zero_span()),
                        file: changed_root,
                    },
                ],
                explain: Some(explain),
            })
        })
        .collect();

    // Deterministic ordering independent of HashMap iteration: score
    // desc, then title asc (matches the digest assembly's own
    // tie-break rule so this provider's own multi-item ordering is
    // consistent with the final rendered order even before assembly's
    // pinned/score/check_id/title sort runs).
    items.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ExportKind, ImportKind, ImportedName};
    use cofferdam_core::LineRange;
    use cofferdam_graph::build_canonical_graph;

    fn span_at(line: u32) -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line,
            column: 1,
        }
    }

    fn named_import(from: &Path, to: &Path, source_name: &str) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./m".to_string(),
            resolved: Some(to.to_path_buf()),
            names: vec![ImportedName {
                source_name: source_name.to_string(),
                local_name: source_name.to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span_at(1),
        }
    }

    fn named_export(file: &Path, name: &str, line: u32) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::Named,
            type_only: false,
            span: span_at(line),
            source_specifier: None,
            resolved_source: None,
        }
    }

    #[test]
    fn direct_caller_of_changed_symbol_ranks_above_plain_importer() {
        let lib = PathBuf::from("/proj/lib.ts");
        let caller = PathBuf::from("/proj/caller.ts");
        let bystander = PathBuf::from("/proj/bystander.ts");

        let exports = vec![named_export(&lib, "doThing", 3)];
        let imports = vec![
            named_import(&caller, &lib, "doThing"),
            named_import(&bystander, &lib, "otherThing"),
        ];
        let g = build_canonical_graph(&imports, &exports);

        let mut changeset = ChangeSet::from_files([lib.clone()]);
        changeset
            .line_ranges
            .insert(lib.clone(), vec![LineRange { start: 3, end: 3 }]);

        let items = compute_blast_radius(&g, &exports, &imports, &changeset);
        assert_eq!(items.len(), 2);
        assert!(items[0].title.contains("caller.ts"));
        assert!(items[0].score > items[1].score);
        assert!(items[0]
            .explain
            .as_deref()
            .unwrap()
            .contains("changed lib.ts"));
    }

    #[test]
    fn transitive_importer_scores_lower_than_direct() {
        let lib = PathBuf::from("/proj/lib.ts");
        let mid = PathBuf::from("/proj/mid.ts");
        let outer = PathBuf::from("/proj/outer.ts");

        let exports = vec![named_export(&lib, "doThing", 1)];
        let imports = vec![
            named_import(&mid, &lib, "doThing"),
            named_import(&outer, &mid, "reexported"),
        ];
        let g = build_canonical_graph(&imports, &exports);
        let changeset = ChangeSet::from_files([lib.clone()]);

        let items = compute_blast_radius(&g, &exports, &imports, &changeset);
        assert_eq!(items.len(), 2);
        let mid_item = items.iter().find(|i| i.title.contains("mid.ts")).unwrap();
        let outer_item = items.iter().find(|i| i.title.contains("outer.ts")).unwrap();
        assert!(mid_item.score > outer_item.score);
        assert_eq!(
            outer_item.explain.as_deref().unwrap(),
            "outer.ts imports mid.ts imports changed lib.ts"
        );
    }

    #[test]
    fn test_file_is_annotated_and_boosted() {
        let lib = PathBuf::from("/proj/lib.ts");
        let test_file = PathBuf::from("/proj/lib.test.ts");

        let exports = vec![named_export(&lib, "doThing", 1)];
        let imports = vec![named_import(&test_file, &lib, "doThing")];
        let g = build_canonical_graph(&imports, &exports);
        let changeset = ChangeSet::from_files([lib.clone()]);

        let items = compute_blast_radius(&g, &exports, &imports, &changeset);
        assert_eq!(items.len(), 1);
        assert!(items[0].body.contains("(test file)"));
    }

    #[test]
    fn unrelated_file_never_surfaces() {
        let lib = PathBuf::from("/proj/lib.ts");
        let unrelated = PathBuf::from("/proj/unrelated.ts");
        let exports = vec![named_export(&lib, "doThing", 1)];
        // unrelated.ts imports nothing from lib.ts at all.
        let imports = vec![named_import(
            &unrelated,
            &PathBuf::from("/proj/other.ts"),
            "x",
        )];
        let g = build_canonical_graph(&imports, &exports);
        let changeset = ChangeSet::from_files([lib.clone()]);

        let items = compute_blast_radius(&g, &exports, &imports, &changeset);
        assert!(items.iter().all(|i| !i.title.contains("unrelated.ts")));
    }

    #[test]
    fn empty_changeset_yields_no_items() {
        let g = build_canonical_graph(&[], &[]);
        let items = compute_blast_radius(&g, &[], &[], &ChangeSet::default());
        assert!(items.is_empty());
    }

    #[test]
    fn depth_bound_is_respected() {
        // A chain longer than MAX_DEPTH — the outermost file must not
        // appear.
        let mut files: Vec<PathBuf> = (0..(MAX_DEPTH + 2))
            .map(|i| PathBuf::from(format!("/proj/f{i}.ts")))
            .collect();
        files.reverse(); // f_last imports ... imports f0 (changed)
        let exports = vec![named_export(&files[files.len() - 1], "x", 1)];
        let mut imports = Vec::new();
        for w in files.windows(2) {
            // w[0] imports from w[1]; chain built so files[len-1] is
            // the changed root and files[0] is furthest out.
            imports.push(named_import(&w[0], &w[1], "x"));
        }
        let g = build_canonical_graph(&imports, &exports);
        let root = files[files.len() - 1].clone();
        let changeset = ChangeSet::from_files([root]);

        let items = compute_blast_radius(&g, &exports, &imports, &changeset);
        let outermost = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(items.iter().all(|i| !i.title.contains(&outermost)));
    }

    #[test]
    fn determinism_across_repeated_runs() {
        let lib = PathBuf::from("/proj/lib.ts");
        let a = PathBuf::from("/proj/a_caller.ts");
        let b = PathBuf::from("/proj/b_caller.ts");
        let t = PathBuf::from("/proj/lib.test.ts");
        let exports = vec![named_export(&lib, "doThing", 1)];
        let imports = vec![
            named_import(&a, &lib, "doThing"),
            named_import(&b, &lib, "otherThing"),
            named_import(&t, &lib, "doThing"),
        ];
        let g = build_canonical_graph(&imports, &exports);
        let changeset = ChangeSet::from_files([lib.clone()]);

        let baseline = compute_blast_radius(&g, &exports, &imports, &changeset);
        let baseline_titles: Vec<String> = baseline.iter().map(|i| i.title.clone()).collect();
        for _ in 0..10 {
            let items = compute_blast_radius(&g, &exports, &imports, &changeset);
            let titles: Vec<String> = items.iter().map(|i| i.title.clone()).collect();
            assert_eq!(titles, baseline_titles);
        }
    }
}
