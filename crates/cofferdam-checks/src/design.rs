//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
#[cfg(test)]
use crate::public_api::is_glob_pattern;
use crate::public_api::{resolve_public_api, PublicApi};
#[cfg(test)]
use cofferdam_core::graph::ImportKind;
use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportRecord, InvariantsRuntime, LayersConfig,
    EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS, INVARIANTS as GRAPH_INVARIANTS,
    LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile, Span,
};
#[cfg(test)]
use cofferdam_graph::build_canonical_graph;
use cofferdam_graph::{normalized_file_path, EdgeKind, Graph, Value, CANONICAL_GRAPH};
use oxc_ast::ast::{
    ArrowFunctionExpression, Declaration, ExportNamedDeclaration, Function, VariableDeclaration,
};
use oxc_ast_visit::Visit;
use smol_str::SmolStr;

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
    autofix: false,
    pure_run: true,
};

impl MaxParameters {
    /// Construct with a parameter-count ceiling. `all_builtins`
    /// installs the default of 5; user config overrides via
    /// `[checks."Design.MaxParameters"].limit`.
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
                location: Location::from_span(&self.file.path, span),
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

/// `Design.DuplicateExportName` — finalize-stage cross-file check
/// that flags identifier names exported from more than one file. See
/// `CheckMeta` for the full emission rules.
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
    autofix: false,
    // Writes per-file ExportRecords into the EXPORTS slot during run();
    // skipping run() on cache hit would drop those contributions and
    // break the cross-file dedupe in finalize(). cp3's corpus-snapshot
    // replay lifts this restriction.
    pure_run: false,
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
                    location: Location::from_span(&e.file, e.span),
                    file: e.file,
                })
                .collect();
            issues.push(Issue {
                check_id: DEN_META.id.to_string(),
                message: format!("`{}` is exported from {} files", name, related.len() + 1),
                file: primary.file.clone(),
                location: Location::from_span(&primary.file, primary.span),
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
//   * Default exports (matched against `import x from './m'` plus
//     `export { default } from './m'` / `import { default as X } from './m'`).
//   * Re-export edges: the engine records `export { x } from './m'` as a
//     virtual Named import edge (cd-klp), so a primitive re-exported through
//     a barrel is reachable whenever the barrel itself is imported under
//     the right name. Multi-level barrels work the same way — each level
//     contributes its own per-name edge.
//   * Type-only exports skipped by default (configurable) — `import type`
//     resolution gets noisy without proper type-aware analysis.
// Out of scope:
//   * Live-set reachability filtering (orphan barrels still shielding their
//     primitives via the per-edge `touched` set) — see cd-ef1's territory.
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
    autofix: false,
    pure_run: true,
};

/// `Design.OrphanExport` — finalize-stage check that flags exports
/// nothing in the project imports. Honours `[public_api]` exports
/// (those ARE the published surface and are exempt). See `CheckMeta`
/// for the contrast with `Refactor.DeadExport`.
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

        // cd-9hp.9 cp3 — graph-backed query path. The engine
        // populates CANONICAL_GRAPH after pass 1 finishes; OrphanExport
        // walks it instead of joining the flat IMPORTS / EXPORTS
        // tables itself. EXPORTS is still consumed here for the
        // per-export reporting payload (span, file, name) — the graph
        // doesn't yet carry export sites. DeadExport / ImportCycle /
        // LayerViolation will migrate behind this check.
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        // [public_api] from cofferdam.invariants.toml. None when no
        // spec was loaded — the per-export skip below becomes a no-op
        // and existing exemption logic (test_patterns,
        // framework_entry_patterns) is the only filter.
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let public_api = runtime
            .as_ref()
            .map(|r| resolve_public_api(&r.public_api.exports, &r.project_root))
            .unwrap_or_default();

        ctx.corpus.with_slot(&CANONICAL_GRAPH, |graph| {
            compute_orphans_on_graph(graph, &exports, &opts, &public_api)
        })
    }
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

/// Test-only entry point that builds an ephemeral canonical graph
/// from flat-table records before delegating to the production
/// graph-query path. Production code (`Design.OrphanExport::finalize`)
/// reads the engine-built `CANONICAL_GRAPH` slot directly and calls
/// [`compute_orphans_on_graph`].
#[cfg(test)]
fn compute_orphans(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    opts: &OrphanOptions,
    public_api: &PublicApi,
) -> Vec<Issue> {
    let graph = build_canonical_graph(imports, exports);
    compute_orphans_on_graph(&graph, exports, opts, public_api)
}

/// Per-file consumption summary read off a target file's incoming
/// import edges. `ns_touched` fires when at least one incoming edge
/// is a namespace import (consumes every named export); `default`
/// when an incoming edge claims the default export; `named` is the
/// set of source-name claims against named exports.
///
/// Default-claim routing mirrors the cd-klp engine fix:
/// `export { default } from './m'` and `import { default as X } from
/// './m'` both surface as Named edges with `source_name == "default"`
/// and must be routed into the default bucket rather than the named
/// bucket.
struct FileConsumption {
    ns_touched: bool,
    default: bool,
    named: HashSet<SmolStr>,
}

/// Walk the incoming import edges on a single file node and reduce
/// them to a [`FileConsumption`] summary. Edge attributes (set by
/// [`cofferdam_graph::build_canonical_graph`]):
///
/// - `import_kind` ∈ `{"default", "named", "namespace",
///   "side_effect"}` — picks the consumption bucket.
/// - `source_name` — the binding's name in the target module
///   (`"default"` for default imports, `"*"` for namespace).
///
/// `side_effect` edges contribute nothing — they don't consume any
/// export.
fn summarise_incoming(g: &Graph, file_node: cofferdam_graph::NodeId) -> FileConsumption {
    let mut out = FileConsumption {
        ns_touched: false,
        default: false,
        named: HashSet::new(),
    };
    for (_src, kind, attrs) in g.incoming(file_node) {
        // Only import edges contribute to consumption. ExportsAs /
        // BelongsToLayer / future Extension edges are noise here.
        if !matches!(kind, EdgeKind::ImportsAsValue | EdgeKind::ImportsAsType) {
            continue;
        }
        let import_kind = match attrs.get(&SmolStr::new_static("import_kind")) {
            Some(Value::String(s)) => s.as_str(),
            _ => continue,
        };
        let source_name = match attrs.get(&SmolStr::new_static("source_name")) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        match import_kind {
            "namespace" => out.ns_touched = true,
            "default" => out.default = true,
            "named" => match source_name {
                Some(name) if name.as_str() == "default" => out.default = true,
                Some(name) => {
                    out.named.insert(name);
                }
                None => {}
            },
            "side_effect" => {}
            _ => {}
        }
    }
    out
}

/// Walk every exporting file and emit one finding per export that
/// the canonical graph shows no consumer for. The bulk of the work
/// is the per-file [`summarise_incoming`] reduction; this function
/// owns the file-level skip rules (test patterns, framework-entry
/// patterns, `[public_api]` allowlist) and the per-export
/// kind-specific consumption rules.
fn compute_orphans_on_graph(
    g: &Graph,
    exports: &[ExportRecord],
    opts: &OrphanOptions,
    public_api: &PublicApi,
) -> Vec<Issue> {
    // Group exports by normalised file path so each file's
    // consumption summary is computed once and shared across its
    // exports.
    let mut by_file: HashMap<String, Vec<&ExportRecord>> = HashMap::new();
    for e in exports {
        by_file.entry(path_key(&e.file)).or_default().push(e);
    }

    let mut issues = Vec::new();
    for (file_key, file_exports) in by_file {
        let mut sorted = file_exports.clone();
        sorted.sort_by_key(|e| e.span.start_byte);

        let file_path = sorted[0].file.clone();
        if matches_substring(&file_path, &opts.test_patterns)
            || matches_substring(&file_path, &opts.framework_entry_patterns)
            || public_api.is_match(&file_key)
        {
            continue;
        }

        let normalised = normalized_file_path(&file_path);
        let consumption = match g.node_id_for_path(&normalised) {
            Some(id) => summarise_incoming(g, id),
            // File never made it into the graph — happens only when
            // no record at all referenced it. Treat as zero
            // consumption (every named/default export is orphan).
            None => FileConsumption {
                ns_touched: false,
                default: false,
                named: HashSet::new(),
            },
        };

        for exp in sorted {
            // Re-exports are forwarding nodes, not orphan candidates.
            if matches!(exp.kind, ExportKind::ReExport) {
                continue;
            }
            if exp.type_only && !opts.include_type_only {
                continue;
            }
            if consumption.ns_touched && matches!(exp.kind, ExportKind::Named) {
                continue;
            }
            let consumed = match exp.kind {
                ExportKind::Default => consumption.default,
                ExportKind::Named => consumption.named.contains(&SmolStr::new(&exp.name)),
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
                    location: Location::from_span(&exp.file, exp.span),
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
            .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
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
    autofix: false,
    pure_run: true,
};

/// `Design.LayerViolation` — finalize-stage check that flags
/// imports crossing the `[layers]` boundaries declared in
/// `cofferdam.invariants.toml`. See `CheckMeta` for the allowlist
/// semantics.
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
            location: Location::from_span(&imp.from_file, imp.span),
            priority: Priority(LV_META.base_priority),
            severity: LV_META.default_severity,
            related: Vec::new(),
        });
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
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
    autofix: false,
    // Reads GRAPH_INVARIANTS from corpus in run() — that slot is
    // populated from cofferdam.invariants.toml so its content is fully
    // captured by config_hash. Could in principle be marked pure once
    // the engine guarantees the config-derived slots are settled
    // before pass 1; for cp2 we stay conservative.
    pure_run: false,
};

/// `Design.BoundaryFrozen` — per-file check that flags any change
/// inside a `[boundaries."path"]` block declared `frozen = true` in
/// `cofferdam.invariants.toml`. See `CheckMeta`.
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
                location: Location::from_span(
                    &file.path,
                    Span {
                        start_byte: 0,
                        end_byte: 0,
                        line: 1,
                        column: 1,
                    },
                ),
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
    autofix: false,
    pure_run: true,
};

/// `Design.InvariantViolation` — finalize-stage check that
/// evaluates `[invariants."rule-name"]` rules from
/// `cofferdam.invariants.toml` against the project's import graph.
/// See `CheckMeta` for the rule-evaluation semantics.
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
                        location: Location::from_span(&imp.from_file, imp.span),
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
                    location: Location::from_span(file, first.span),
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

// ─── Design.ScriptedInvariant ──────────────────────────────────────────────
//
// Evaluates each `[invariants.scripted."rule-name"]` block from
// cofferdam.invariants.toml via the v1 predicate DSL (cd-9hp.1).
// Per-file gate (`when`) + body (`require`/`forbid`); on failure,
// emit one finding per (file, rule) with the rule's `message`.
//
// File enumeration: per-file `run()` records every TS path into a
// corpus slot; `finalize()` reads back and evaluates each rule across
// the recorded set. Files with no imports/exports are still seen.
//
// Span semantics (v1): file-level (line 1, column 1). Per-edge spans
// for `imports` predicates are reserved for a future MINOR bump.

use cofferdam_core::dsl::ast::TopPredicate;
use cofferdam_core::dsl::evaluator::{eval_predicate, eval_top, EvalCtx};
use cofferdam_core::dsl::parser::parse_predicate;
use cofferdam_core::ScriptedInvariantSpec;

const SI_META: CheckMeta = CheckMeta {
    id: "Design.ScriptedInvariant",
    category: Category::Design,
    base_priority: 7,
    default_severity: Severity::Medium,
    explanation: "A scripted invariant declared in cofferdam.invariants.toml under [invariants.scripted] is violated for this file.",
    body: include_str!("../docs/Design.ScriptedInvariant.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes per-file evidence to SCRIPTED_FILES in run() so finalize
    // can iterate files where the `when` gate matched. Skipping run()
    // on cache hit would drop that bookkeeping and silently break the
    // scripted-invariant pipeline.
    pure_run: false,
};

/// Corpus slot recording every parsed file `Design.ScriptedInvariant` saw
/// in pass 1. `finalize` reads back and iterates the set so files with no
/// imports/exports are still checked.
static SCRIPTED_FILES: CorpusKey<Vec<PathBuf>> = CorpusKey::new("Design.ScriptedInvariant.files");

/// `Design.ScriptedInvariant` — evaluates user-declared scripted rules
/// from `[invariants.scripted.*]` against the project graph. See
/// `CheckMeta` for the rule shape and the v1 DSL surface.
pub struct ScriptedInvariant;

impl Check for ScriptedInvariant {
    fn meta(&self) -> &'static CheckMeta {
        &SI_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // Record the file path so finalize can iterate the universe of
        // engine-seen TS files (not just files with imports/exports).
        ctx.corpus.with_slot(&SCRIPTED_FILES, |slot| {
            slot.push(file.path.clone());
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let Some(runtime) = runtime else {
            return Vec::new();
        };
        if runtime.scripted.is_empty() {
            return Vec::new();
        }

        // Re-parse each rule. Strings were validated at config load
        // (`invariants::parse`), so failures here are an internal
        // invariant: degrade gracefully by skipping the offending rule
        // rather than panicking.
        let compiled: Vec<CompiledScriptedRule> = runtime
            .scripted
            .iter()
            .filter_map(|(name, spec)| CompiledScriptedRule::compile(name, spec))
            .collect();
        if compiled.is_empty() {
            return Vec::new();
        }

        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        let layers: Option<LayersConfig> = ctx.corpus.with_slot(&GRAPH_LAYERS, |slot| slot.clone());

        // Universe: files this check saw plus any file mentioned as an
        // import source / export site (defensive — `run` is supposed to
        // see every TS file but the union costs nothing).
        let mut seen: HashSet<PathBuf> = HashSet::new();
        ctx.corpus.with_slot(&SCRIPTED_FILES, |slot| {
            for p in slot.drain(..) {
                seen.insert(p);
            }
        });
        for imp in &imports {
            seen.insert(imp.from_file.clone());
        }
        for exp in &exports {
            seen.insert(exp.file.clone());
        }
        let mut files: Vec<PathBuf> = seen.into_iter().collect();
        files.sort();

        // Group imports/exports by from_file / file for cheap per-file
        // EvalCtx construction.
        let mut imports_by_file: HashMap<PathBuf, Vec<ImportRecord>> = HashMap::new();
        for imp in imports {
            imports_by_file
                .entry(imp.from_file.clone())
                .or_default()
                .push(imp);
        }
        let mut exports_by_file: HashMap<PathBuf, Vec<ExportRecord>> = HashMap::new();
        for exp in exports {
            exports_by_file
                .entry(exp.file.clone())
                .or_default()
                .push(exp);
        }

        let project_root = runtime.project_root.clone();
        let empty_imports: Vec<ImportRecord> = Vec::new();
        let empty_exports: Vec<ExportRecord> = Vec::new();

        let mut issues = Vec::new();
        for file in &files {
            let file_imports = imports_by_file.get(file).unwrap_or(&empty_imports);
            let file_exports = exports_by_file.get(file).unwrap_or(&empty_exports);
            let eval_ctx = EvalCtx {
                file_path: file,
                project_root: &project_root,
                imports: file_imports,
                exports: file_exports,
                layers: layers.as_ref(),
            };

            for rule in &compiled {
                // `when` gate. Treat eval errors as "rule does not apply"
                // for this file rather than crashing the whole pass —
                // surfaces as no finding from a buggy gate, which is
                // strictly better than a panic.
                if let Some(when) = &rule.when {
                    match eval_predicate(when, &eval_ctx) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(_) => continue,
                    }
                }
                let passes = match eval_top(&rule.body, &eval_ctx) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if passes {
                    continue;
                }
                issues.push(Issue {
                    check_id: SI_META.id.to_string(),
                    message: format!(
                        "scripted invariant `{}` failed: {}",
                        rule.name, rule.message
                    ),
                    file: file.clone(),
                    location: Location::from_span(
                        file,
                        Span {
                            start_byte: 0,
                            end_byte: 0,
                            line: 1,
                            column: 1,
                        },
                    ),
                    priority: Priority(SI_META.base_priority),
                    severity: SI_META.default_severity,
                    related: Vec::new(),
                });
            }
        }

        issues.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.message.cmp(&b.message))
        });
        issues
    }
}

/// One scripted rule with its predicates pre-parsed. `body` carries
/// the chosen field as a `TopPredicate`:
/// * `require X` → `TopPredicate::Require(X)`
/// * `forbid X`  → `TopPredicate::Forbid(X)`
struct CompiledScriptedRule {
    name: String,
    when: Option<cofferdam_core::dsl::ast::Predicate>,
    body: TopPredicate,
    message: String,
}

impl CompiledScriptedRule {
    fn compile(name: &str, spec: &ScriptedInvariantSpec) -> Option<Self> {
        let when = match spec.when.as_deref() {
            Some(src) => Some(parse_predicate(src).ok()?),
            None => None,
        };
        let body = if let Some(src) = spec.require.as_deref() {
            TopPredicate::Require(Box::new(parse_predicate(src).ok()?))
        } else if let Some(src) = spec.forbid.as_deref() {
            TopPredicate::Forbid(Box::new(parse_predicate(src).ok()?))
        } else {
            return None;
        };
        Some(Self {
            name: name.to_string(),
            when,
            body,
            message: spec.message.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Helper: build a PublicApi from a slice of entry strings using
    // `project_root` as the resolved root, exercising `resolve_public_api`.
    fn make_public_api(entries: &[&str], project_root: &Path) -> PublicApi {
        let owned: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        resolve_public_api(&owned, project_root)
    }

    // Helper: normalise a path the same way the engine does for
    // `ExportRecord.file` — absolutize first (cd-q9f), then forward-slash
    // and lower-case on Windows (`path_key`). The absolutize step matches
    // what `Engine.analyze_with_sources` does to every input path, so
    // these tests mirror the real wire shape rather than the raw key()
    // form. cd-gro / gh #41 turned on the analogous absolutize in
    // resolve_public_api; key() must do the same on both sides or the
    // tests would diverge from production.
    fn key(p: &Path) -> String {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        path_key(&abs)
    }

    #[test]
    fn exact_path_matches() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/index.ts"], &root);
        let file_key = key(&root.join("src/index.ts"));
        assert!(api.is_match(&file_key), "exact path should match");
    }

    #[test]
    fn exact_path_no_spurious_match() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/index.ts"], &root);
        let other = key(&root.join("src/other.ts"));
        assert!(
            !api.is_match(&other),
            "unrelated file should not match exact entry"
        );
    }

    #[test]
    fn dot_slash_prefix_stripped() {
        // "./src/index.ts" must resolve the same as "src/index.ts".
        let root = PathBuf::from("/project");
        let api = make_public_api(&["./src/index.ts"], &root);
        let file_key = key(&root.join("src/index.ts"));
        assert!(
            api.is_match(&file_key),
            "./src/index.ts should strip ./ and match"
        );
    }

    #[test]
    fn glob_matches_multiple_files() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["components/ui/**/*.tsx"], &root);

        let button = key(&root.join("components/ui/button.tsx"));
        let nested = key(&root.join("components/ui/forms/input.tsx"));
        let outside = key(&root.join("components/other/widget.tsx"));

        assert!(
            api.is_match(&button),
            "glob should match direct child *.tsx"
        );
        assert!(api.is_match(&nested), "glob should match nested *.tsx");
        assert!(
            !api.is_match(&outside),
            "glob should not match outside the tree"
        );
    }

    #[test]
    fn glob_star_single_level() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/*.ts"], &root);

        let direct = key(&root.join("src/index.ts"));
        let nested = key(&root.join("src/sub/other.ts"));

        assert!(
            api.is_match(&direct),
            "single-level glob should match direct child"
        );
        assert!(
            !api.is_match(&nested),
            "single-level glob should not match nested path"
        );
    }

    #[test]
    fn empty_entries_matches_nothing() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&[], &root);
        let key = path_key(&root.join("src/anything.ts"));
        assert!(!api.is_match(&key), "empty allowlist should not match");
    }

    #[test]
    fn package_json_entry_is_ignored() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["package.json:exports"], &root);
        // No file key should match — the entry is silently dropped.
        let key = path_key(&root.join("src/index.ts"));
        assert!(!api.is_match(&key));
    }

    #[test]
    fn invalid_glob_does_not_panic() {
        // An unclosed bracket `[` is an invalid glob. resolve_public_api
        // must skip it rather than panicking.
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/[invalid"], &root);
        // Nothing should match, but also no panic.
        let key = path_key(&root.join("src/anything.ts"));
        assert!(!api.is_match(&key));
    }

    #[test]
    fn exact_and_glob_coexist() {
        let root = PathBuf::from("/project");
        // Mix: one exact path + one glob.
        let api = make_public_api(&["src/index.ts", "components/ui/**/*.tsx"], &root);

        let exact_key = key(&root.join("src/index.ts"));
        let glob_key = key(&root.join("components/ui/button.tsx"));
        let miss = key(&root.join("src/other.ts"));

        assert!(
            api.is_match(&exact_key),
            "exact entry should still match when globs present"
        );
        assert!(api.is_match(&glob_key), "glob entry should match");
        assert!(!api.is_match(&miss), "unrelated file should not match");
    }

    #[test]
    fn is_glob_pattern_detects_metacharacters() {
        assert!(is_glob_pattern("**/*.tsx"));
        assert!(is_glob_pattern("src/*.ts"));
        assert!(is_glob_pattern("src/[abc].ts"));
        assert!(is_glob_pattern("{a,b}.ts"));
        assert!(!is_glob_pattern("src/index.ts"));
        assert!(!is_glob_pattern("./src/index.ts"));
    }

    // cd-gro / gh #41: when the invariants spec is discovered from a
    // relative root, project_root is relative. The engine absolutizes
    // every source path via `std::path::absolute` (cd-q9f), so the
    // file_key passed to `is_match` is always absolute. Before the fix,
    // the exact entry stored a relative key and `exact.contains(absolute)`
    // silently missed every entry — the user's gh #41 symptom.
    //
    // Both halves are pinned here: exact-path lookup and the glob-path
    // root-prefix stripping that lets `apps/foo/**` match an absolute
    // `/cwd/apps/foo/x.ts`.
    #[test]
    fn relative_project_root_normalises_to_absolute_for_exact_match() {
        // Use the process CWD as the absolutize anchor — that's what
        // `std::path::absolute(".")` resolves against. The test asserts
        // the relative-project-root path produces the same exact key
        // as the absolute-project-root path would.
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from(".");
        let api = make_public_api(&["apps/web/src/App.tsx"], &rel_root);

        // Engine emits file paths as absolute (cd-q9f).
        let absolute_file = cwd.join("apps/web/src/App.tsx");
        let file_key = path_key(&absolute_file);
        assert!(
            api.is_match(&file_key),
            "exact entry with relative project_root must still match an absolute file_key — \
             cd-gro / gh #41. file_key={file_key}",
        );
    }

    #[test]
    fn relative_project_root_normalises_to_absolute_for_glob_match() {
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from(".");
        let api = make_public_api(&["apps/web/src/routes/**"], &rel_root);

        let absolute_file = cwd.join("apps/web/src/routes/index.tsx");
        let file_key = path_key(&absolute_file);
        assert!(
            api.is_match(&file_key),
            "glob entry with relative project_root must still match an absolute file_key — \
             cd-gro / gh #41. file_key={file_key}",
        );
    }

    #[test]
    fn dot_subdir_project_root_works_too() {
        // `./apps` style — relative but with a subdir segment. Also tests
        // that the absolutize step doesn't choke on a relative prefix.
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from("./apps");
        let api = make_public_api(&["web/src/App.tsx"], &rel_root);

        let absolute_file = cwd.join("apps").join("web/src/App.tsx");
        let file_key = path_key(&absolute_file);
        assert!(api.is_match(&file_key));
    }

    // ── compute_orphans: re-export edge reachability (cd-klp) ──────────────

    use cofferdam_core::graph::ImportedName;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        }
    }

    fn opts_default() -> OrphanOptions {
        OrphanOptions {
            include_type_only: false,
            test_patterns: Vec::new(),
            framework_entry_patterns: Vec::new(),
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

    fn default_export(file: &Path) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: "default".to_string(),
            kind: ExportKind::Default,
            type_only: false,
            span: span(),
            source_specifier: None,
            resolved_source: None,
        }
    }

    fn reexport(file: &Path, name: &str, source: &Path) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::ReExport,
            type_only: false,
            span: span(),
            source_specifier: Some("./".to_string()),
            resolved_source: Some(source.to_path_buf()),
        }
    }

    fn named_import(
        from: &Path,
        resolved: &Path,
        source_name: &str,
        local_name: &str,
    ) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
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

    fn default_import(from: &Path, resolved: &Path, local_name: &str) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: "default".to_string(),
                local_name: local_name.to_string(),
                kind: ImportKind::Default,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    fn namespace_import(from: &Path, resolved: &Path) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: "*".to_string(),
                local_name: "ns".to_string(),
                kind: ImportKind::Namespace,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    #[test]
    fn shadcn_barrel_consumed_via_named_reexport() {
        // dialog.tsx → index.ts (re-exports Dialog) → app.ts (imports Dialog).
        // The engine records the re-export as an ImportRecord with
        // resolved=dialog.tsx, source_name="Dialog", which puts
        // (dialog.tsx, "Dialog") in `touched`. dialog.tsx's Dialog
        // export must NOT be flagged as orphan.
        let dialog = PathBuf::from("/p/components/ui/dialog.tsx");
        let index = PathBuf::from("/p/components/ui/index.ts");
        let app = PathBuf::from("/p/app/page.tsx");

        let imports = vec![
            named_import(&index, &dialog, "Dialog", "Dialog"),
            named_import(&app, &index, "Dialog", "Dialog"),
        ];
        let exports = vec![
            named_export(&dialog, "Dialog"),
            reexport(&index, "Dialog", &dialog),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        assert!(
            issues.is_empty(),
            "Dialog should be reachable through the barrel; got: {:?}",
            issues
        );
    }

    #[test]
    fn aliased_named_reexport_consumed() {
        // export { Dialog as MyDialog } from './dialog' → engine records
        // an import with source_name="Dialog", local_name="MyDialog".
        // touched gets (dialog, "Dialog"), which matches dialog.tsx's
        // export name.
        let dialog = PathBuf::from("/p/dialog.tsx");
        let index = PathBuf::from("/p/index.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&index, &dialog, "Dialog", "MyDialog"),
            named_import(&app, &index, "MyDialog", "MyDialog"),
        ];
        let exports = vec![
            named_export(&dialog, "Dialog"),
            reexport(&index, "MyDialog", &dialog),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        assert!(
            issues.is_empty(),
            "Aliased re-export should reach the source name; got: {:?}",
            issues
        );
    }

    #[test]
    fn multi_level_named_reexport_consumed() {
        // primitive.ts → inner_barrel.ts (re-exports X) → outer_barrel.ts
        // (re-exports X) → app.ts (imports X). Each level contributes
        // its own per-edge ImportRecord, so primitive.ts:X stays
        // reachable through transitive barrels.
        let primitive = PathBuf::from("/p/primitive.ts");
        let inner = PathBuf::from("/p/inner.ts");
        let outer = PathBuf::from("/p/outer.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&inner, &primitive, "X", "X"),
            named_import(&outer, &inner, "X", "X"),
            named_import(&app, &outer, "X", "X"),
        ];
        let exports = vec![
            named_export(&primitive, "X"),
            reexport(&inner, "X", &primitive),
            reexport(&outer, "X", &inner),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        assert!(
            issues.is_empty(),
            "Multi-level barrels should chain; got: {:?}",
            issues
        );
    }

    #[test]
    fn dead_barrel_no_longer_shields_primitive() {
        // primitive.ts is re-exported from dead_barrel.ts but nobody
        // consumes dead_barrel.ts. Today's per-edge `touched` does
        // record (primitive, "PrimitiveX") because of the re-export
        // edge from dead_barrel — so primitive is NOT flagged. That's
        // the documented limit of this fix (out-of-scope: live-set
        // reachability filtering). The point of this test is to pin
        // the behavior so a future stricter implementation surfaces
        // here as an intentional change.
        let primitive = PathBuf::from("/p/primitive.ts");
        let barrel = PathBuf::from("/p/dead_barrel.ts");

        let imports = vec![named_import(
            &barrel,
            &primitive,
            "PrimitiveX",
            "PrimitiveX",
        )];
        let exports = vec![
            named_export(&primitive, "PrimitiveX"),
            reexport(&barrel, "PrimitiveX", &primitive),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        // primitive is shielded by the per-edge touch (acknowledged
        // false-negative); but the previous lenient `reexport_sources`
        // shortcut also shielded the case where the barrel was missing
        // the per-name edge. See the OUT OF SCOPE note in the module.
        assert_eq!(
            issues.len(),
            0,
            "primitive.ts:PrimitiveX is touched by the re-export edge; got: {:?}",
            issues
        );
    }

    #[test]
    fn export_default_from_named_reexport() {
        // index.ts: `export { default } from './dialog'` plus a
        // consumer doing `import D from './index'`. The engine records
        // the re-export as Named with source_name="default"; the
        // compute_orphans loop maps that into default_touched so
        // dialog.tsx's default export reads as consumed.
        let dialog = PathBuf::from("/p/dialog.tsx");
        let index = PathBuf::from("/p/index.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&index, &dialog, "default", "default"),
            default_import(&app, &index, "D"),
        ];
        let exports = vec![
            default_export(&dialog),
            reexport(&index, "default", &dialog),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        assert!(
            issues.is_empty(),
            "`export {{ default }} from` should reach the source default; got: {:?}",
            issues
        );
    }

    #[test]
    fn star_reexport_marks_namespace_consumed() {
        // `export * from './m'` records a Namespace ImportRecord, so
        // every named export of m reads as consumed.
        let m = PathBuf::from("/p/m.ts");
        let barrel = PathBuf::from("/p/barrel.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            namespace_import(&barrel, &m),
            named_import(&app, &barrel, "X", "X"),
        ];
        let exports = vec![
            named_export(&m, "X"),
            named_export(&m, "Y"),
            reexport(&barrel, "*", &m),
        ];
        let issues = compute_orphans(&imports, &exports, &opts_default(), &PublicApi::default());
        assert!(
            issues.is_empty(),
            "namespace re-export should consume all named exports; got: {:?}",
            issues
        );
    }

    #[test]
    fn unused_named_export_still_flagged() {
        // Sanity check: when nothing imports a named export, it's
        // still flagged. Guards against the fix accidentally turning
        // the check into a no-op.
        let m = PathBuf::from("/p/m.ts");
        let exports = vec![named_export(&m, "Forgotten")];
        let issues = compute_orphans(&[], &exports, &opts_default(), &PublicApi::default());
        assert_eq!(issues.len(), 1, "expected one orphan, got: {:?}", issues);
        assert!(issues[0].message.contains("Forgotten"));
    }
}
