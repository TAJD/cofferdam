//! Storage-backend spike for the canonical graph (cd-acv3 / cd-9hp.9.1).
//!
//! Validates that petgraph + indexed attribute maps is competitive with
//! the existing flat-table corpus shape for the workloads cd-9hp.9
//! cares about:
//!
//! 1. **Graph build cost** — after the engine populates the IMPORTS /
//!    EXPORTS corpus slots, how long does it take to materialise a
//!    petgraph `DiGraph` plus a `path → NodeId` index from those slots?
//!    Should be linear in (imports + exports) and small in absolute
//!    terms.
//!
//! 2. **Direct-edge query** — "does file F directly import module M?"
//!    Cofferdam's existing checks answer this by iterating the flat
//!    IMPORTS slice. The graph alternative looks up F's outgoing edges
//!    via the path index.
//!
//! 3. **Transitive closure query** — "does file F transitively import
//!    module M?" at bounded depths (2 / 4 / 8). This is the workload
//!    the petgraph-vs-Datalog trade-off hinges on. petgraph offers BFS
//!    / DFS / topological algorithms; Datalog gives us declarative
//!    closure. The spike measures what petgraph actually costs.
//!
//! # Scope guardrails
//!
//! - Tiny slice of the cd-9hp.9 schema: only `File` / `Import` /
//!   `Export` nodes and `ImportsAsValue` / `ImportsAsType` /
//!   `ExportsAs` edges. No `Layer`, no `Symbol`, no namespaced
//!   `Extension`.
//! - No engine wiring. The spike runs ONLY post-analysis as a
//!   benchmarking harness.
//! - No ADR rewriting from inside this file; the decision lives in
//!   `docs/decisions/0001-canonical-graph-schema.md`.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p cofferdam-engine --release --example graph_spike -- <dir>
//! ```
//!
//! Multiple directories may be passed; each is measured separately and
//! printed as one block.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use cofferdam_checks::all_builtins;
use cofferdam_core::graph::{ExportKind, ExportRecord, ImportKind, ImportRecord};
use cofferdam_core::{CorpusIndex, EXPORTS, IMPORTS};
use cofferdam_engine::Engine;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

// ---------------------------------------------------------------------------
// Spike-only schema slice
// ---------------------------------------------------------------------------

/// A subset of `NodeKind` from cd-9hp.9's design notes — just enough to
/// represent the import/export graph for benchmarking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum NodeKind {
    File(PathBuf),
    Import { specifier: String },
    Export { name: String, file: PathBuf },
}

/// A subset of `EdgeKind`. We collapse type/value into one variant with
/// a flag because the spike doesn't query on the distinction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EdgeKind {
    Imports { type_only: bool },
    Exports(ExportKind),
}

// ---------------------------------------------------------------------------
// Materialised graph + index
// ---------------------------------------------------------------------------

struct GraphBundle {
    graph: DiGraph<NodeKind, EdgeKind>,
    path_index: HashMap<PathBuf, NodeIndex>,
}

impl GraphBundle {
    fn build(imports: &[ImportRecord], exports: &[ExportRecord]) -> Self {
        let mut graph: DiGraph<NodeKind, EdgeKind> = DiGraph::new();
        let mut path_index: HashMap<PathBuf, NodeIndex> = HashMap::new();
        let mut import_index: HashMap<String, NodeIndex> = HashMap::new();

        // Pass 1: every file mentioned as a source OR destination becomes
        // a node. Resolves to a stable NodeIndex via path_index.
        for imp in imports {
            ensure_file(&mut graph, &mut path_index, &imp.from_file);
            if let Some(resolved) = &imp.resolved {
                ensure_file(&mut graph, &mut path_index, resolved);
            }
        }
        for exp in exports {
            ensure_file(&mut graph, &mut path_index, &exp.file);
        }

        // Pass 2: edges. Imports edge from importing-file → resolved file
        // when resolution succeeded; otherwise the spike skips the edge
        // (the real schema would represent unresolved imports as edges
        // into an `Import { specifier }` node, but the spike only
        // benchmarks in-project closure so external specifiers don't
        // change the answer).
        for imp in imports {
            let src = *path_index
                .get(&imp.from_file)
                .expect("from_file always registered");
            let dst = match &imp.resolved {
                Some(p) => match path_index.get(p) {
                    Some(idx) => *idx,
                    None => continue,
                },
                None => {
                    // External specifier — represented as a per-specifier
                    // Import node so direct-edge queries still hit. We
                    // share one node per distinct specifier across
                    // importing files.
                    *import_index
                        .entry(imp.source_specifier.clone())
                        .or_insert_with(|| {
                            graph.add_node(NodeKind::Import {
                                specifier: imp.source_specifier.clone(),
                            })
                        })
                }
            };
            // One graph-level edge per `ImportRecord`; per-name type-only
            // bindings get folded into the statement-level type_only
            // flag for spike purposes.
            graph.add_edge(
                src,
                dst,
                EdgeKind::Imports {
                    type_only: imp.type_only
                        || imp.names.iter().all(|n| n.type_only) && !imp.names.is_empty(),
                },
            );
        }
        for exp in exports {
            let src = *path_index
                .get(&exp.file)
                .expect("export file always registered");
            // Export records become nodes anchored at the exporting file.
            // The spike's queries care about "file F exports name N",
            // so we don't bother with a shared symbol node.
            let exp_node = graph.add_node(NodeKind::Export {
                name: exp.name.clone(),
                file: exp.file.clone(),
            });
            graph.add_edge(src, exp_node, EdgeKind::Exports(exp.kind));
        }

        GraphBundle { graph, path_index }
    }

    /// True iff `from` has a direct edge to a file whose path equals
    /// `target_path` or to an `Import` node whose specifier equals
    /// `target_specifier`. The flat-table baseline does the same work
    /// by iterating ImportRecords.
    fn direct_imports(&self, from: &Path, target_specifier: &str) -> bool {
        let Some(src) = self.path_index.get(from) else {
            return false;
        };
        for edge in self.graph.edges(*src) {
            match &self.graph[edge.target()] {
                NodeKind::File(p) => {
                    if p.to_string_lossy().contains(target_specifier) {
                        return true;
                    }
                }
                NodeKind::Import { specifier } => {
                    if specifier == target_specifier {
                        return true;
                    }
                }
                NodeKind::Export { .. } => continue,
            }
        }
        false
    }

    /// True iff `from` transitively imports any node whose `File` path
    /// or `Import` specifier matches `target_specifier`, bounded to
    /// `max_depth` hops. Uses petgraph's BFS via manual frontier
    /// expansion so we can cap depth.
    fn transitively_imports(&self, from: &Path, target_specifier: &str, max_depth: usize) -> bool {
        let Some(start) = self.path_index.get(from).copied() else {
            return false;
        };
        let mut visited: std::collections::HashSet<NodeIndex> = std::collections::HashSet::new();
        let mut frontier: Vec<NodeIndex> = vec![start];
        visited.insert(start);
        let mut depth = 0;
        while !frontier.is_empty() && depth < max_depth {
            let mut next: Vec<NodeIndex> = Vec::new();
            for node in frontier {
                for edge in self.graph.edges(node) {
                    if !matches!(edge.weight(), EdgeKind::Imports { .. }) {
                        continue;
                    }
                    let tgt = edge.target();
                    if !visited.insert(tgt) {
                        continue;
                    }
                    match &self.graph[tgt] {
                        NodeKind::File(p) => {
                            if p.to_string_lossy().contains(target_specifier) {
                                return true;
                            }
                            next.push(tgt);
                        }
                        NodeKind::Import { specifier } => {
                            if specifier == target_specifier {
                                return true;
                            }
                            // Import nodes have no outgoing edges in this
                            // spike, but be defensive.
                            next.push(tgt);
                        }
                        NodeKind::Export { .. } => continue,
                    }
                }
            }
            frontier = next;
            depth += 1;
        }
        false
    }

    fn node_count(&self) -> usize {
        self.graph.node_count()
    }
    fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

fn ensure_file(
    graph: &mut DiGraph<NodeKind, EdgeKind>,
    path_index: &mut HashMap<PathBuf, NodeIndex>,
    p: &Path,
) {
    if path_index.contains_key(p) {
        return;
    }
    let idx = graph.add_node(NodeKind::File(p.to_path_buf()));
    path_index.insert(p.to_path_buf(), idx);
}

// ---------------------------------------------------------------------------
// Flat-table baseline (the current cofferdam shape)
// ---------------------------------------------------------------------------

/// Direct-edge lookup against the flat IMPORTS slice — the existing
/// shape `Design.LayerViolation`, `Design.InvariantViolation` and the
/// DSL evaluator's `imports` operator use.
fn flat_direct_imports(imports: &[ImportRecord], from: &Path, target_specifier: &str) -> bool {
    imports.iter().any(|imp| {
        if imp.from_file != from {
            return false;
        }
        if imp.source_specifier == target_specifier {
            return true;
        }
        if let Some(r) = &imp.resolved {
            return r.to_string_lossy().contains(target_specifier);
        }
        false
    })
}

/// Transitive closure on the flat shape — what cd-9hp.1's v1 evaluator
/// would have to grow if we stayed flat. Open-coded BFS over the
/// ImportRecord slice, indexing by from_file on the fly.
fn flat_transitively_imports(
    imports: &[ImportRecord],
    from: &Path,
    target_specifier: &str,
    max_depth: usize,
) -> bool {
    // One-shot index: from_file → its imports. The flat path would have
    // to rebuild this on every query if we don't cache, which is the
    // honest comparison against petgraph's pre-built adjacency.
    let mut by_file: HashMap<&Path, Vec<&ImportRecord>> = HashMap::new();
    for imp in imports {
        by_file
            .entry(imp.from_file.as_path())
            .or_default()
            .push(imp);
    }
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut frontier: Vec<PathBuf> = vec![from.to_path_buf()];
    visited.insert(from.to_path_buf());
    let mut depth = 0;
    while !frontier.is_empty() && depth < max_depth {
        let mut next = Vec::new();
        for f in &frontier {
            let Some(edges) = by_file.get(f.as_path()) else {
                continue;
            };
            for imp in edges {
                if imp.source_specifier == target_specifier {
                    return true;
                }
                if let Some(r) = &imp.resolved {
                    if r.to_string_lossy().contains(target_specifier) {
                        return true;
                    }
                    if visited.insert(r.clone()) {
                        next.push(r.clone());
                    }
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Spike driver
// ---------------------------------------------------------------------------

fn run_one(dir: &Path) {
    println!("\n=== {} ===", dir.display());

    // Discover sources the same way the CLI does.
    let opts = cofferdam_engine::DiscoveryOptions::default();
    let sources = match cofferdam_engine::discover(&[dir], &opts) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("discovery failed: {e}");
            return;
        }
    };
    println!("discovered {} source files", sources.len());

    // Drive a full analyze to populate IMPORTS / EXPORTS in the engine's
    // corpus. The spike doesn't care about the issues — just the
    // populated slots — but engine.analyze() doesn't expose the corpus,
    // so we re-build the graph extractor against a fresh corpus by
    // running the regular check pipeline and copying out.
    //
    // Cheapest path: use the public analyze() and rebuild import/export
    // records the same way the engine does. We can't directly read the
    // engine's CorpusIndex (it's internal to analyze_with_sources), so
    // we use a separate corpus + the engine's graph::GraphBuilder.
    // Simpler still: re-walk the source files through Engine::analyze
    // and read corpus slots through a public hook.
    //
    // The simplest robust approach for a spike: read sources from disk,
    // run Engine::analyze to populate findings, ignore findings, then
    // re-extract imports/exports by running the engine again but
    // intercepting the corpus.
    //
    // Even simpler: use the public corpus types and trigger a second
    // analyze_with_sources path that exposes the corpus via a flag. We
    // don't have such a flag — so for the spike we build a "fake" corpus
    // by running our own per-file extractor. The engine's graph builder
    // lives behind a `pub(crate)` boundary today; we work around it by
    // running engine.analyze() to validate things parse and then
    // separately call out to a public helper.
    //
    // For the spike's purposes, the canonical surface IS already public
    // enough: Engine::analyze populates IMPORTS/EXPORTS into its
    // internal corpus, which we can't directly read. Workaround: add a
    // tiny test-only public API for the spike, OR (cleaner) ship the
    // spike as a #[test] that uses Engine::analyze and then
    // re-creates the imports list from a manually-driven graph
    // extractor. We chose the second.
    //
    // Concretely: in this spike we lean on the public `analyze` to read
    // sources end-to-end, and then re-run the import/export extraction
    // by directly running the engine through a small wrapper that
    // exposes the corpus after analyze. Today that means re-doing a
    // small amount of work, but it's a spike — correctness over
    // micro-optimisation.

    let engine = Engine::new(all_builtins());
    let t0 = Instant::now();
    let analyze_result =
        engine.analyze(&sources.iter().map(|p| p.to_path_buf()).collect::<Vec<_>>());
    let analyze_elapsed = t0.elapsed();
    let issue_count = analyze_result.as_ref().map(|v| v.len()).unwrap_or(0);
    println!(
        "engine.analyze: {:?} ({} findings)",
        analyze_elapsed, issue_count
    );

    // Re-extract imports/exports through a fresh engine to a corpus we
    // own. We use the public test-friendly path via the engine's
    // `analyze_with_sources` indirectly by replicating its graph
    // extraction step here. To avoid coupling the spike to engine
    // internals, we read the sources ourselves and use the public
    // graph::GraphBuilder via cofferdam_engine::graph (which IS public
    // — see crates/cofferdam-engine/src/lib.rs `pub mod graph`).
    let corpus = CorpusIndex::new();
    let builder = cofferdam_engine::graph::GraphBuilder::new();
    let allocator = cofferdam_core::Allocator::default();
    let mut total_parse = std::time::Duration::ZERO;
    let mut total_extract = std::time::Duration::ZERO;
    for path in &sources {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let file = cofferdam_core::SourceFile::new(path.clone(), text);
        if file.language != cofferdam_core::Language::TypeScript {
            continue;
        }
        let t_parse = Instant::now();
        let parsed_return = cofferdam_core::parser::parse_into(&allocator, &file);
        total_parse += t_parse.elapsed();
        if cofferdam_core::parser::parse_fatal(&parsed_return) {
            continue;
        }
        let parsed = cofferdam_core::ParsedView {
            program: &parsed_return.program,
            diagnostics: &parsed_return.errors,
        };
        let t_extract = Instant::now();
        builder.collect(&file, &parsed, &corpus);
        total_extract += t_extract.elapsed();
    }
    println!(
        "parse loop:    {:?}  (graph extract within: {:?})",
        total_parse, total_extract
    );

    // Snapshot the corpus slices.
    let imports: Vec<ImportRecord> = corpus.with_slot(&IMPORTS, |s| s.to_vec());
    let exports: Vec<ExportRecord> = corpus.with_slot(&EXPORTS, |s| s.to_vec());
    println!("imports = {}, exports = {}", imports.len(), exports.len());

    // ===== Measurement 1: graph materialisation =====
    let t_build = Instant::now();
    let bundle = GraphBundle::build(&imports, &exports);
    let build_elapsed = t_build.elapsed();
    println!(
        "petgraph build: {:?}  ({} nodes, {} edges)",
        build_elapsed,
        bundle.node_count(),
        bundle.edge_count()
    );

    // Pick query targets: 32 random (from_file, specifier) pairs drawn
    // from the imports themselves so every query has at least a chance
    // of hitting. We use the first 32 distinct ImportRecord (from_file,
    // source_specifier) tuples — deterministic, no rand dependency.
    let mut queries: Vec<(PathBuf, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(PathBuf, String)> = std::collections::HashSet::new();
    for imp in &imports {
        let key = (imp.from_file.clone(), imp.source_specifier.clone());
        if seen.insert(key.clone()) {
            queries.push(key);
            if queries.len() == 32 {
                break;
            }
        }
    }
    if queries.is_empty() {
        println!("(no import edges to query — fixture too small?)");
        return;
    }

    // ===== Measurement 2: direct-edge query =====
    bench("petgraph direct", queries.len(), || {
        let mut hits = 0;
        for (from, spec) in &queries {
            if bundle.direct_imports(from, spec) {
                hits += 1;
            }
        }
        hits
    });
    bench("flat-table direct", queries.len(), || {
        let mut hits = 0;
        for (from, spec) in &queries {
            if flat_direct_imports(&imports, from, spec) {
                hits += 1;
            }
        }
        hits
    });

    // ===== Measurement 3: closure at depths 2 / 4 / 8 =====
    for depth in [2usize, 4, 8] {
        bench(
            &format!("petgraph closure d={}", depth),
            queries.len(),
            || {
                let mut hits = 0;
                for (from, spec) in &queries {
                    if bundle.transitively_imports(from, spec, depth) {
                        hits += 1;
                    }
                }
                hits
            },
        );
        bench(
            &format!("flat-table closure d={}", depth),
            queries.len(),
            || {
                let mut hits = 0;
                for (from, spec) in &queries {
                    if flat_transitively_imports(&imports, from, spec, depth) {
                        hits += 1;
                    }
                }
                hits
            },
        );
    }
}

fn bench(label: &str, query_count: usize, mut f: impl FnMut() -> usize) {
    // 5 iterations: median guards against cold-cache noise.
    let mut runs: Vec<std::time::Duration> = Vec::with_capacity(5);
    let mut hits = 0;
    for _ in 0..5 {
        let t = Instant::now();
        hits = f();
        runs.push(t.elapsed());
    }
    runs.sort();
    let median = runs[runs.len() / 2];
    let per_q = median / (query_count as u32);
    println!(
        "  {:<28} median {:>10?}  ({:>6?}/query, {} hits)",
        label, median, per_q, hits
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: graph_spike <dir>...\n\nExample:\n  cargo run -p cofferdam-engine --release --example graph_spike -- C:/Users/tajdi/bestefforttools"
        );
        std::process::exit(2);
    }
    for arg in args {
        run_one(Path::new(&arg));
    }
}

// Suppress dead_code on EdgeKind variants that the spike doesn't query
// today. Keeping them around documents the slice of cd-9hp.9's edge
// taxonomy we modelled.
#[allow(dead_code)]
fn _silence_warnings(k: EdgeKind, ek: ExportKind, ik: ImportKind) {
    let _ = (k, ek, ik);
}
