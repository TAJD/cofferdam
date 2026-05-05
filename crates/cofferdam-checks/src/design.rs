//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportKind, ImportRecord, InvariantsRuntime, LayersConfig,
    EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS, INVARIANTS as GRAPH_INVARIANTS,
    LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, OptionDefault,
    OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, Declaration, ExportNamedDeclaration, Function, VariableDeclaration,
};
use oxc_ast_visit::Visit;

/// `Design.MaxParameters` — flag function signatures over `limit` params.
///
/// Counts logical parameter slots (an `{a, b}` destructure is 1, a `...rest`
/// is 1, a default-valued param is 1). Cheap counter check that exercises
/// the same SDK seam as TripleEquals from a different angle (visiting
/// function-like nodes rather than expressions).
pub struct MaxParameters {
    limit: u32,
    meta: &'static CheckMeta,
}

const MP_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(5),
    doc: "maximum number of parameters per function signature",
}];

const META: CheckMeta = CheckMeta {
    id: "Design.MaxParameters",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "Functions with too many parameters are hard to call correctly. Pass an options object instead.",
    body: include_str!("../docs/Design.MaxParameters.md"),
    requires_types: false,
    consistency: false,
    options: MP_OPTIONS,
};

impl MaxParameters {
    pub fn new(limit: u32) -> Self {
        Self { limit, meta: &META }
    }
}

impl Check for MaxParameters {
    fn meta(&self) -> &'static CheckMeta {
        self.meta
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let limit = ctx
            .options
            .get_int("limit")
            .map(|v| v as u32)
            .unwrap_or(self.limit);
        let mut visitor = Collector {
            file,
            limit,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
}

impl<'a> Collector<'a> {
    fn check_params(&mut self, count: usize, name: &str, span_start: u32, span_end: u32) {
        if count as u32 > self.limit {
            let span = span_from_bytes(&self.file.text, span_start, span_end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "{} has {} parameters, exceeds limit of {}",
                    name, count, self.limit
                ),
                file: self.file.path.clone(),
                span,
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        self.check_params(
            node.params.items.len(),
            &name,
            node.span.start,
            node.span.end,
        );
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        self.check_params(
            node.params.items.len(),
            "arrow function",
            node.span.start,
            node.span.end,
        );
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }
}

// ─── Design.DuplicateExportName ────────────────────────────────────────────
//
// First cross-file check using the cd-0ps corpus API. Flags the same exported
// identifier appearing in 2+ files (named exports of functions/classes/
// const/let/var). Common cause: barrel re-exports collide silently when two
// modules export `helper` or `parseDate`.
//
// Coverage at v1: `export function/class/const/let/var <ident>`. Specifier-
// only exports (`export { x as y }`) and `export default <expr>` are
// follow-ups; the corpus API + finalize plumbing is identical for them.

/// One observed export, collected during the per-file pass.
#[derive(Clone)]
struct NamedExport {
    name: String,
    file: PathBuf,
    span: Span,
}

/// Per-process slot in the corpus. All `DuplicateExportName` instances share
/// it (only one is registered today, but the API permits sharing).
static EXPORTS: CorpusKey<Vec<NamedExport>> = CorpusKey::new("Design.DuplicateExportName.exports");

pub struct DuplicateExportName;

const DEN_META: CheckMeta = CheckMeta {
    id: "Design.DuplicateExportName",
    category: Category::Design,
    base_priority: 6,
    default_severity: Severity::Medium,
    explanation: "The same name is exported from multiple files. Barrel re-exports collide silently and importers can't tell which one they got.",
    body: include_str!("../docs/Design.DuplicateExportName.md"),
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for DuplicateExportName {
    fn meta(&self) -> &'static CheckMeta {
        &DEN_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = ExportCollector {
            file,
            collected: Vec::new(),
        };
        visitor.visit_program(parsed.program);

        // Hand off to the shared corpus. The lock is held only for the
        // length of this drain — every other check's per-file work
        // continues uncontended.
        ctx.corpus.with_slot(&EXPORTS, |slot| {
            slot.append(&mut visitor.collected);
        });

        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let mut by_name: std::collections::BTreeMap<String, Vec<NamedExport>> =
            std::collections::BTreeMap::new();
        ctx.corpus.with_slot(&EXPORTS, |slot| {
            for exp in slot.drain(..) {
                by_name.entry(exp.name.clone()).or_default().push(exp);
            }
        });

        let mut issues = Vec::new();
        for (name, mut occurrences) in by_name {
            if occurrences.len() < 2 {
                continue;
            }
            // Stable order: first-seen by (file, start_byte). The smallest
            // (path, offset) becomes the primary; the rest are `related`.
            occurrences.sort_by(|a, b| {
                a.file
                    .cmp(&b.file)
                    .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
            });
            let primary = occurrences.remove(0);
            let related: Vec<RelatedSpan> = occurrences
                .into_iter()
                .map(|e| RelatedSpan {
                    file: e.file,
                    span: e.span,
                })
                .collect();
            issues.push(Issue {
                check_id: DEN_META.id.to_string(),
                message: format!("`{}` is exported from {} files", name, related.len() + 1),
                file: primary.file,
                span: primary.span,
                priority: Priority(DEN_META.base_priority),
                severity: Severity::Medium,
                related,
            });
        }
        issues
    }
}

// ─── Design.OrphanExport ───────────────────────────────────────────────────
//
// First consumer of the project graph (cd-7h5). Reads pass-1 IMPORTS and
// EXPORTS slots in `finalize`, flags any export whose (file, name) pair is
// never imported anywhere in the project. Test files are excluded from
// export analysis so test-only helpers don't get flagged; they DO count as
// import sites so their consumption keeps real exports off the orphan list.
//
// Coverage:
//   * Named exports (`export function/class/const/let/var`, `export { x }`).
//   * Default exports (matched against `import x from './m'`).
//   * Type-only exports skipped by default (configurable) — `import type`
//     resolution gets noisy without proper type-aware analysis.
// Out of scope (separate child beads):
//   * Re-export chain walking — re-export records aren't flagged here, but
//     the underlying export IS evaluated (and rightly flagged if no leaf
//     consumer touches its name). Walking through barrels to attribute the
//     orphan to the deepest definition is cd-ef1 territory.
//   * package.json `main`/`module`/`exports` entry-point allowlist.

#[derive(Debug, Clone)]
struct OrphanOptions {
    include_type_only: bool,
    test_patterns: Vec<String>,
    framework_entry_patterns: Vec<String>,
}

const OE_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "include_type_only",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(false),
        doc: "Flag type-only exports (interfaces, type aliases, declared types) as orphans.",
    },
    OptionSpec {
        name: "test_file_patterns",
        kind: OptionKind::StringList,
        default: OptionDefault::StringList(&[
            ".test.",
            ".spec.",
            "_test.",
            "_spec.",
            "/__tests__/",
            "/__mocks__/",
        ]),
        doc: "Filename substrings that mark a file as test/mocks. Exports from matching files are skipped.",
    },
    OptionSpec {
        name: "framework_entry_patterns",
        kind: OptionKind::StringList,
        // Source-of-truth list lives in `crate::framework_paths` so
        // `Warning.UnusedImport` shares it (cd-q53). Substring-matched
        // against the forward-slash-normalized path.
        default: OptionDefault::StringList(FRAMEWORK_ENTRY_PATTERNS),
        doc: "Filename substrings for framework entry-point files (Next.js App Router, Pages Router, SvelteKit, config files). Exports from matching files are skipped because the framework runtime — not application code — consumes them.",
    },
];

const OE_META: CheckMeta = CheckMeta {
    id: "Design.OrphanExport",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "An exported symbol is never imported anywhere in the project. Likely dead code left over from a refactor.",
    body: include_str!("../docs/Design.OrphanExport.md"),
    requires_types: false,
    consistency: false,
    options: OE_OPTIONS,
};

pub struct OrphanExport;

impl Check for OrphanExport {
    fn meta(&self) -> &'static CheckMeta {
        &OE_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // All work happens in finalize — the engine populates IMPORTS/
        // EXPORTS in pass 1 before any check sees the file, so per-file
        // run() has nothing to add.
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        // Resolved options come through FinalizeContext (cd-3uj) so
        // user overrides in cofferdam.toml take effect. The engine
        // populates every declared key with at least its schema
        // default before finalize runs, so the unwrap_or_default
        // arms only fire if a future refactor drops a key.
        let opts = OrphanOptions {
            include_type_only: ctx.options.get_bool("include_type_only").unwrap_or(false),
            test_patterns: ctx
                .options
                .get_string_list("test_file_patterns")
                .map(|xs| xs.to_vec())
                .unwrap_or_default(),
            framework_entry_patterns: ctx
                .options
                .get_string_list("framework_entry_patterns")
                .map(|xs| xs.to_vec())
                .unwrap_or_default(),
        };

        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        // [public_api] from cofferdam.invariants.toml. None when no
        // spec was loaded — the per-export skip below becomes a no-op
        // and existing exemption logic (test_patterns,
        // framework_entry_patterns) is the only filter.
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let public_api_files = runtime
            .as_ref()
            .map(|r| resolve_public_api(&r.public_api.exports, &r.project_root))
            .unwrap_or_default();

        compute_orphans(&imports, &exports, &opts, &public_api_files)
    }
}

/// Convert `[public_api].exports` entries into a set of normalised file
/// keys for fast lookup. Today only direct file paths are honoured —
/// `package.json:<key>` entries parse but are no-ops (follow-up bead
/// will resolve them via the package's exports map). Empty input yields
/// an empty set; OrphanExport's caller treats that as "no allowlist".
fn resolve_public_api(entries: &[String], project_root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    for entry in entries {
        if entry.starts_with("package.json:") {
            // Resolution deferred — see follow-up bead. Listed here so
            // the schema accepts it; today these strings are dropped.
            continue;
        }
        let trimmed = entry.trim_start_matches("./");
        let abs = project_root.join(trimmed);
        out.insert(path_key(&abs));
    }
    out
}

fn matches_substring(path: &Path, patterns: &[String]) -> bool {
    let s = path.to_string_lossy();
    let normalized = s.replace('\\', "/");
    patterns.iter().any(|p| normalized.contains(p))
}

/// Build a normalized path key by lowercasing on Windows (case-insensitive
/// filesystem) and using as-is on case-sensitive platforms. Avoids spurious
/// orphans when oxc_resolver returns `C:\Foo` and discovery returned
/// `C:\foo`.
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn compute_orphans(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    opts: &OrphanOptions,
    public_api_files: &HashSet<String>,
) -> Vec<Issue> {
    // Set of (resolved_path_key, source_name) tuples: every named touch.
    let mut touched: HashSet<(String, String)> = HashSet::new();
    // Set of resolved_path_keys reached via `import * as ns` — these
    // touch every named export of the target file.
    let mut namespace_touched: HashSet<String> = HashSet::new();
    // Set of resolved_path_keys whose default was imported.
    let mut default_touched: HashSet<String> = HashSet::new();
    // Set of files re-exported from somewhere — we treat re-export sources
    // as touched (the re-exporter is the consumer; whether anyone uses the
    // re-export is the re-exporter's problem, not the source's).
    let mut reexport_sources: HashSet<String> = HashSet::new();

    for imp in imports {
        let Some(resolved) = &imp.resolved else {
            continue;
        };
        let key = path_key(resolved);
        for n in &imp.names {
            match n.kind {
                ImportKind::Default => {
                    default_touched.insert(key.clone());
                }
                ImportKind::Namespace => {
                    namespace_touched.insert(key.clone());
                }
                ImportKind::Named => {
                    touched.insert((key.clone(), n.source_name.clone()));
                }
            }
        }
    }
    for exp in exports {
        if let Some(src) = &exp.resolved_source {
            reexport_sources.insert(path_key(src));
        }
    }

    // Group exports by file so we can attribute "namespace touched" once.
    let mut by_file: HashMap<String, Vec<&ExportRecord>> = HashMap::new();
    for e in exports {
        by_file.entry(path_key(&e.file)).or_default().push(e);
    }

    let mut issues = Vec::new();
    for (file_key, file_exports) in by_file {
        // Sort within file by start_byte for deterministic ordering.
        let mut sorted = file_exports.clone();
        sorted.sort_by_key(|e| e.span.start_byte);

        let file_path = sorted[0].file.clone();
        if matches_substring(&file_path, &opts.test_patterns)
            || matches_substring(&file_path, &opts.framework_entry_patterns)
            || public_api_files.contains(&file_key)
        {
            continue;
        }

        let ns_seen = namespace_touched.contains(&file_key) || reexport_sources.contains(&file_key);

        for exp in sorted {
            // Re-export records are forwarding nodes, not orphan candidates.
            if matches!(exp.kind, ExportKind::ReExport) {
                continue;
            }
            if exp.type_only && !opts.include_type_only {
                continue;
            }
            // Treat namespace-touched and re-export-sourced files as having
            // every named export consumed.
            if ns_seen {
                continue;
            }
            let consumed = match exp.kind {
                ExportKind::Default => default_touched.contains(&file_key),
                ExportKind::Named => touched.contains(&(file_key.clone(), exp.name.clone())),
                ExportKind::ReExport => true,
            };
            if !consumed {
                let display_name = if matches!(exp.kind, ExportKind::Default) {
                    "default export".to_string()
                } else {
                    format!("`{}`", exp.name)
                };
                issues.push(Issue {
                    check_id: OE_META.id.to_string(),
                    message: format!(
                        "{} is exported but never imported in the project",
                        display_name
                    ),
                    file: exp.file.clone(),
                    span: exp.span,
                    priority: Priority(OE_META.base_priority),
                    severity: OE_META.default_severity,
                    related: Vec::new(),
                });
            }
        }
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
    });
    issues
}

struct ExportCollector<'a> {
    file: &'a SourceFile,
    collected: Vec<NamedExport>,
}

impl<'a> ExportCollector<'a> {
    fn record(&mut self, name: &str, start: u32, end: u32) {
        let span = span_from_bytes(&self.file.text, start, end);
        self.collected.push(NamedExport {
            name: name.to_string(),
            file: self.file.path.clone(),
            span,
        });
    }

    fn record_var_decl(&mut self, var: &VariableDeclaration<'a>) {
        for d in &var.declarations {
            if let Some(ident) = d.id.get_binding_identifier() {
                self.record(ident.name.as_str(), ident.span.start, ident.span.end);
            }
        }
    }
}

impl<'a> Visit<'a> for ExportCollector<'a> {
    fn visit_export_named_declaration(&mut self, node: &ExportNamedDeclaration<'a>) {
        if let Some(decl) = &node.declaration {
            match decl {
                Declaration::FunctionDeclaration(f) => {
                    if let Some(id) = &f.id {
                        self.record(id.name.as_str(), id.span.start, id.span.end);
                    }
                }
                Declaration::ClassDeclaration(c) => {
                    if let Some(id) = &c.id {
                        self.record(id.name.as_str(), id.span.start, id.span.end);
                    }
                }
                Declaration::VariableDeclaration(v) => {
                    self.record_var_decl(v);
                }
                _ => {}
            }
        }
        oxc_ast_visit::walk::walk_export_named_declaration(self, node);
    }
}

// ─── Design.ImportCycle ────────────────────────────────────────────────────
//
// Detects circular import chains via Tarjan's strongly-connected-component
// algorithm over the in-project import graph. A cycle is any SCC of size
// ≥ 2 (mutual / longer cycle) or a single node with a self-loop. Type-only
// imports are excluded by default — TS allows clean type-only cycles via
// `import type` and they don't cause runtime issues.
//
// Output: one finding per cycle, anchored at the alphabetically-lowest
// member; other members appear in `related`. Cycles are stable across
// runs because the canonical order is filename-sorted.

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
    body: include_str!("../docs/Design.ImportCycle.md"),
    requires_types: false,
    consistency: false,
    options: IC_OPTIONS,
};

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
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
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
                RelatedSpan {
                    file: display[id].clone(),
                    span,
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
            file: display[primary_id].clone(),
            span: primary_span,
            priority: Priority(IC_META.base_priority),
            severity: IC_META.default_severity,
            related,
        });
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
    });
    issues
}

// ─── Design.LayerViolation ─────────────────────────────────────────────────
//
// Reads the `[layers]` table from cofferdam.toml (published into the
// LAYERS corpus slot by the engine). Each in-project file is mapped to
// at most one layer via gitignore-style globs against its path-relative-
// to-project-root. For every import edge whose source layer is declared,
// check whether the destination layer appears in the source's `allow`
// list; if not, emit a finding.
//
// Files that don't match any declared layer are ignored — the user
// hasn't said how to think about them. Files in a layer with no `allow`
// entry MAY only import from themselves (sane default for an isolated
// layer). Same-layer edges are always permitted.

const LV_META: CheckMeta = CheckMeta {
    id: "Design.LayerViolation",
    category: Category::Design,
    base_priority: 9,
    default_severity: Severity::High,
    explanation: "An import crosses a declared architectural layer in a direction not permitted by [layers].allow.",
    body: include_str!("../docs/Design.LayerViolation.md"),
    requires_types: false,
    consistency: false,
    options: &[],
};

pub struct LayerViolation;

impl Check for LayerViolation {
    fn meta(&self) -> &'static CheckMeta {
        &LV_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let cfg: Option<LayersConfig> = ctx.corpus.with_slot(&GRAPH_LAYERS, |slot| slot.clone());
        let Some(cfg) = cfg else {
            return Vec::new();
        };
        if cfg.layers.is_empty() {
            return Vec::new();
        }
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        compute_layer_violations(&cfg, &imports)
    }
}

fn compute_layer_violations(cfg: &LayersConfig, imports: &[ImportRecord]) -> Vec<Issue> {
    let matchers = cofferdam_core::layers::build_matchers(cfg);
    if matchers.is_empty() {
        return Vec::new();
    }

    // Cache file → layer once so both endpoints of each edge are looked
    // up at most once per file across all of its imports.
    let mut layer_of: HashMap<String, Option<String>> = HashMap::new();
    let mut resolve_layer = |path: &Path| -> Option<String> {
        let key = path_key(path);
        if let Some(cached) = layer_of.get(&key) {
            return cached.clone();
        }
        let layer = cofferdam_core::layers::layer_for(&matchers, &cfg.project_root, path);
        layer_of.insert(key, layer.clone());
        layer
    };

    let mut issues = Vec::new();
    for imp in imports {
        if imp.type_only {
            // Type-only edges don't affect runtime layering.
            continue;
        }
        let Some(resolved) = &imp.resolved else {
            continue;
        };
        let Some(src_layer) = resolve_layer(&imp.from_file) else {
            continue;
        };
        let Some(dst_layer) = resolve_layer(resolved) else {
            continue;
        };
        if src_layer == dst_layer {
            continue;
        }
        // Empty `allow` list (or absent) means: only same-layer imports
        // permitted. Explicit empty `[]` is the user opting in to that.
        let allowed = cfg
            .allow
            .get(&src_layer)
            .map(|deps| deps.iter().any(|d| d == &dst_layer))
            .unwrap_or(false);
        if allowed {
            continue;
        }
        issues.push(Issue {
            check_id: LV_META.id.to_string(),
            message: format!(
                "layer `{}` may not import from layer `{}`",
                src_layer, dst_layer
            ),
            file: imp.from_file.clone(),
            span: imp.span,
            priority: Priority(LV_META.base_priority),
            severity: LV_META.default_severity,
            related: Vec::new(),
        });
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
    });
    issues
}

/// Iterative Tarjan's SCC algorithm. Returns one Vec per SCC. The
/// recursion-free form avoids blowing the stack on large codebases.
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

// ─── Design.BoundaryFrozen ─────────────────────────────────────────────────
//
// `[boundaries] "src/legacy/**" = { frozen = true, reason = "..." }` in
// cofferdam.invariants.toml signals "no new code in this area". v0
// stub-warns: emits one finding per source file matching a frozen glob,
// with the configured reason in the message. The intended full
// semantic — flag *additions* to the frozen area against a baseline — is
// gated on a follow-up bead. Today's stub gives authors a visible
// reminder; suppress with a directive or a severity override on
// projects where the area is intentionally still active during
// migration.

const BF_META: CheckMeta = CheckMeta {
    id: "Design.BoundaryFrozen",
    category: Category::Design,
    base_priority: 0,
    default_severity: Severity::Low,
    explanation: "File lives inside an architectural boundary marked frozen=true in cofferdam.invariants.toml. New code in this area should be reviewed against the boundary's stated reason.",
    body: include_str!("../docs/Design.BoundaryFrozen.md"),
    requires_types: false,
    consistency: false,
    options: &[],
};

pub struct BoundaryFrozen;

impl Check for BoundaryFrozen {
    fn meta(&self) -> &'static CheckMeta {
        &BF_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let Some(runtime) = runtime else {
            return Vec::new();
        };
        if runtime.boundaries.is_empty() {
            return Vec::new();
        }
        let normalized_rel = relative_normalised(&runtime.project_root, &file.path);
        let mut issues = Vec::new();
        for (glob, spec) in &runtime.boundaries {
            if !spec.frozen {
                continue;
            }
            let Ok(matcher) = globset::Glob::new(glob) else {
                continue;
            };
            if !matcher.compile_matcher().is_match(&normalized_rel) {
                continue;
            }
            let reason = spec
                .reason
                .as_deref()
                .map(|r| format!(": {}", r))
                .unwrap_or_default();
            issues.push(Issue {
                check_id: BF_META.id.to_string(),
                message: format!("file is in frozen boundary `{}`{}", glob, reason),
                file: file.path.clone(),
                span: Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
                priority: Priority(0),
                severity: Severity::Medium, // engine post-pass overwrites with default_severity
                related: Vec::new(),
            });
        }
        issues
    }
}

fn relative_normalised(project_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(project_root).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    // Discovery sometimes hands us paths like `./src/foo.ts`; strip the
    // leading `./` so glob authors don't have to anticipate it. Also
    // strip a leading `/` for the rare case where strip_prefix fell
    // through and we got an absolute-looking string.
    s.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

// ─── Design.InvariantViolation ─────────────────────────────────────────────
//
// Generic forbid-imports / require-imports rule engine. `[invariants]`
// in cofferdam.invariants.toml declares one or more rules, each fired
// independently per import edge. Findings are emitted in `finalize`
// once the project graph is fully populated.
//
// `forbid_imports`: emits a finding at the import statement when the
// importing file is in `from_layers` (or `from_layers` is empty) AND
// the resolved-or-specifier form starts with any forbidden prefix.
//
// `require_imports`: emits one finding per file in `from_layers` when
// the file imports nothing matching any required prefix.

const IV_META: CheckMeta = CheckMeta {
    id: "Design.InvariantViolation",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation:
        "An import edge violates a `[invariants]` rule declared in cofferdam.invariants.toml.",
    body: include_str!("../docs/Design.InvariantViolation.md"),
    requires_types: false,
    consistency: false,
    options: &[],
};

pub struct InvariantViolation;

impl Check for InvariantViolation {
    fn meta(&self) -> &'static CheckMeta {
        &IV_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let Some(runtime) = runtime else {
            return Vec::new();
        };
        if runtime.invariants.is_empty() {
            return Vec::new();
        }
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let layers: Option<LayersConfig> = ctx.corpus.with_slot(&GRAPH_LAYERS, |slot| slot.clone());
        let layer_matchers = layers
            .as_ref()
            .map(cofferdam_core::layers::build_matchers)
            .unwrap_or_default();
        let project_root = runtime.project_root.clone();

        let mut issues = Vec::new();

        // forbid_imports — emit per offending import edge.
        for (name, spec) in &runtime.invariants {
            if spec.forbid_imports.is_empty() {
                continue;
            }
            for imp in &imports {
                if !file_in_layers(
                    &layer_matchers,
                    &project_root,
                    &imp.from_file,
                    &spec.from_layers,
                ) {
                    continue;
                }
                if let Some(forbidden) =
                    matches_any_prefix(&project_root, imp, &spec.forbid_imports)
                {
                    issues.push(Issue {
                        check_id: IV_META.id.to_string(),
                        message: format!(
                            "invariant `{}` violated: forbidden import `{}` (matches prefix `{}`)",
                            name, imp.source_specifier, forbidden
                        ),
                        file: imp.from_file.clone(),
                        span: imp.span,
                        priority: Priority(IV_META.base_priority),
                        severity: Severity::Medium,
                        related: Vec::new(),
                    });
                }
            }
        }

        // require_imports — emit one finding per file-in-layer that has
        // no import matching any required prefix.
        for (name, spec) in &runtime.invariants {
            if spec.require_imports.is_empty() {
                continue;
            }
            // Group imports by from_file once.
            let mut by_file: HashMap<PathBuf, Vec<&ImportRecord>> = HashMap::new();
            for imp in &imports {
                by_file.entry(imp.from_file.clone()).or_default().push(imp);
            }
            // Collect the set of files that need to satisfy the rule —
            // every file with at least one import that's "in layer".
            // (A file outside from_layers is unaffected; a file in
            // from_layers with no imports at all is also unaffected —
            // require_imports speaks to import edges, not to bare
            // file existence.)
            for (file, file_imports) in &by_file {
                if !file_in_layers(&layer_matchers, &project_root, file, &spec.from_layers) {
                    continue;
                }
                let satisfied = file_imports.iter().any(|imp| {
                    matches_any_prefix(&project_root, imp, &spec.require_imports).is_some()
                });
                if satisfied {
                    continue;
                }
                let first = &file_imports[0];
                issues.push(Issue {
                    check_id: IV_META.id.to_string(),
                    message: format!(
                        "invariant `{}` violated: file is missing required import matching one of {:?}",
                        name, spec.require_imports
                    ),
                    file: file.clone(),
                    span: first.span,
                    priority: Priority(IV_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                });
            }
        }

        issues
    }
}

/// True when `file` matches any of `layers` (or `layers` is empty —
/// "applies everywhere"). Uses the LayersConfig matchers built from
/// the merged `[layers]` config. When no layers are declared at all,
/// an empty `from_layers` still matches; a non-empty `from_layers`
/// without a matching layer drops the rule.
fn file_in_layers(
    matchers: &[cofferdam_core::layers::LayerMatcher],
    project_root: &Path,
    file: &Path,
    layers: &[String],
) -> bool {
    if layers.is_empty() {
        return true;
    }
    let Some(layer) = cofferdam_core::layers::layer_for(matchers, project_root, file) else {
        return false;
    };
    layers.iter().any(|l| l == &layer)
}

/// Return the matched prefix when an import's resolved path or
/// specifier starts with one of `prefixes`. Resolved paths are
/// matched against the project-relative, forward-slash form;
/// specifiers are matched verbatim.
fn matches_any_prefix(
    project_root: &Path,
    imp: &ImportRecord,
    prefixes: &[String],
) -> Option<String> {
    let rel: Option<String> = imp
        .resolved
        .as_ref()
        .map(|p| relative_normalised(project_root, p));
    for pfx in prefixes {
        if imp.source_specifier == *pfx || imp.source_specifier.starts_with(&format!("{}/", pfx)) {
            return Some(pfx.clone());
        }
        if let Some(rel) = &rel {
            if rel == pfx || rel.starts_with(&format!("{}/", pfx)) {
                return Some(pfx.clone());
            }
        }
    }
    None
}
