use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use cofferdam_core::graph::{
    ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, OptionDefault,
    OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile, Span,
};

const IC_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "ignore_type_only",
    kind: OptionKind::Bool,
    default: OptionDefault::Bool(true),
    doc: "Skip cycles that exist only via `import type` edges. TS allows clean type-only cycles.",
}];

const IC_META: CheckMeta = CheckMeta {
    id: "Design.ImportCycle",
    category: Category::Design,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "Files in this group import each other in a cycle. Cycles cause initialization-order surprises and obscure module boundaries.",
    body: include_str!("../../docs/Design.ImportCycle.md"),
    requires_types: false,
    consistency: false,
    options: IC_OPTIONS,
    autofix: false,
    pure_run: true,
};

/// `Design.ImportCycle` — finalize-stage check that flags closed
/// cycles in the project's import graph. Self-imports and cycles
/// confined to a single file aren't flagged. See `CheckMeta`.
pub struct ImportCycle;

impl Check for ImportCycle {
    fn meta(&self) -> &'static CheckMeta {
        &IC_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        // Honour user-supplied option override (cd-3uj). Default mirrors
        // the schema (true) when the key is missing.
        let ignore_type_only = ctx.options.get_bool("ignore_type_only").unwrap_or(true);
        let imports: Vec<ImportRecord> = ctx
            .corpus
            .with_slot(&GRAPH_IMPORTS, |slot| slot.records().cloned().collect());
        let exports: Vec<ExportRecord> = ctx
            .corpus
            .with_slot(&GRAPH_EXPORTS, |slot| slot.records().cloned().collect());
        compute_cycles(&imports, &exports, ignore_type_only)
    }
}

/// Build the in-project file universe + per-file import edges, run
/// Tarjan, emit one finding per non-trivial SCC. Public-by-convention so
/// future tests can call it directly without spinning up a corpus.
fn compute_cycles(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    ignore_type_only: bool,
) -> Vec<Issue> {
    // Universe: any path that appeared as a from_file (we parsed it) OR
    // as an export site is "in project". External node_modules paths
    // never appear as from_file so they're naturally excluded.
    let mut universe: HashSet<String> = HashSet::new();
    for imp in imports {
        universe.insert(path_key(&imp.from_file));
    }
    for exp in exports {
        universe.insert(path_key(&exp.file));
    }

    // Stable id assignment: sort the universe alphabetically so SCC ids
    // and cycle anchors are deterministic across runs.
    let mut id_for: HashMap<String, usize> = HashMap::new();
    let mut display: Vec<PathBuf> = Vec::new();
    let mut sorted_universe: Vec<String> = universe.into_iter().collect();
    sorted_universe.sort();
    for (idx, key) in sorted_universe.iter().enumerate() {
        id_for.insert(key.clone(), idx);
    }
    // Recover one display PathBuf per id from the first record seen.
    display.resize(sorted_universe.len(), PathBuf::new());
    for imp in imports {
        let key = path_key(&imp.from_file);
        if let Some(&id) = id_for.get(&key) {
            if display[id].as_os_str().is_empty() {
                display[id] = imp.from_file.clone();
            }
        }
    }
    for exp in exports {
        let key = path_key(&exp.file);
        if let Some(&id) = id_for.get(&key) {
            if display[id].as_os_str().is_empty() {
                display[id] = exp.file.clone();
            }
        }
    }

    // Adjacency: for each src id, the imports it makes into other
    // in-project files, ordered by appearance for stable cycle anchoring.
    // Each edge keeps the originating ImportRecord so we can attach the
    // import-statement span to the finding.
    let mut adj: Vec<Vec<(usize, Span, PathBuf)>> = vec![Vec::new(); sorted_universe.len()];
    for imp in imports {
        if ignore_type_only && imp.type_only {
            continue;
        }
        let Some(resolved) = &imp.resolved else {
            continue;
        };
        let src = match id_for.get(&path_key(&imp.from_file)) {
            Some(&id) => id,
            None => continue,
        };
        let dst = match id_for.get(&path_key(resolved)) {
            Some(&id) => id,
            None => continue, // external (node_modules etc.)
        };
        if src == dst {
            // Self-import — a degenerate "cycle". Record it so Tarjan
            // emits a 1-node SCC with a self-loop (we'll detect via the
            // edge list size).
        }
        adj[src].push((dst, imp.span, imp.from_file.clone()));
    }

    let sccs = tarjan_sccs(&adj);

    // Build issues. Skip 1-node SCCs unless they have a self-edge.
    let mut issues = Vec::new();
    for scc in sccs {
        if scc.len() < 2 {
            let id = scc[0];
            let has_self = adj[id].iter().any(|(dst, _, _)| *dst == id);
            if !has_self {
                continue;
            }
        }
        // Sort SCC members by display path; anchor on the first.
        let mut members = scc.clone();
        members.sort_by(|a, b| display[*a].cmp(&display[*b]));
        let scc_set: HashSet<usize> = members.iter().copied().collect();

        let primary_id = members[0];
        let primary_edge = adj[primary_id]
            .iter()
            .find(|(dst, _, _)| scc_set.contains(dst));
        let primary_file = display[primary_id].clone();
        let primary_span = primary_edge.map(|(_, span, _)| *span).unwrap_or(Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        });

        let related: Vec<RelatedSpan> = members[1..]
            .iter()
            .map(|&id| {
                let span = adj[id]
                    .iter()
                    .find(|(dst, _, _)| scc_set.contains(dst))
                    .map(|(_, s, _)| *s)
                    .unwrap_or(Span {
                        start_byte: 0,
                        end_byte: 0,
                        line: 1,
                        column: 1,
                    });
                let rel_file = display[id].clone();
                RelatedSpan {
                    location: Location::from_span(&rel_file, span),
                    file: rel_file,
                }
            })
            .collect();

        let cycle_len = members.len();
        let message = if cycle_len == 1 {
            "this file imports itself".to_string()
        } else {
            format!("import cycle of {} files", cycle_len)
        };

        issues.push(Issue {
            check_id: IC_META.id.to_string(),
            message,
            file: primary_file.clone(),
            location: Location::from_span(&primary_file, primary_span),
            priority: Priority(IC_META.base_priority),
            severity: IC_META.default_severity,
            related,
        });
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
    });
    issues
}

fn tarjan_sccs(adj: &[Vec<(usize, Span, PathBuf)>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    let mut indices = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut sccs: Vec<Vec<usize>> = Vec::new();
    let mut next_index = 0usize;

    // Per-node DFS state for iteration.
    struct Frame {
        v: usize,
        edge_iter: usize,
    }

    for start in 0..n {
        if indices[start] != usize::MAX {
            continue;
        }
        let mut frames: Vec<Frame> = Vec::new();
        indices[start] = next_index;
        lowlink[start] = next_index;
        next_index += 1;
        stack.push(start);
        on_stack[start] = true;
        frames.push(Frame {
            v: start,
            edge_iter: 0,
        });

        while let Some(frame) = frames.last_mut() {
            let v = frame.v;
            if frame.edge_iter < adj[v].len() {
                let (w, _, _) = adj[v][frame.edge_iter];
                frame.edge_iter += 1;
                if indices[w] == usize::MAX {
                    indices[w] = next_index;
                    lowlink[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    frames.push(Frame { v: w, edge_iter: 0 });
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(indices[w]);
                }
            } else {
                // Finished v's edges. Compare lowlink against its index.
                if lowlink[v] == indices[v] {
                    let mut scc: Vec<usize> = Vec::new();
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        scc.push(w);
                        if w == v {
                            break;
                        }
                    }
                    sccs.push(scc);
                }
                frames.pop();
                if let Some(parent) = frames.last_mut() {
                    lowlink[parent.v] = lowlink[parent.v].min(lowlink[v]);
                }
            }
        }
    }

    sccs
}
