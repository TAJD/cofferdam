//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportKind, ImportRecord, EXPORTS as GRAPH_EXPORTS,
    IMPORTS as GRAPH_IMPORTS,
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
        // Next.js App Router (`page`/`layout`/`route`/`error`/`loading`/...
        // are loaded by the framework, not user code), Pages Router
        // (`_app`, `_document`), Next/Vite/Vitest config files, and
        // SvelteKit `+page.svelte` / `+layout.ts` style routing.
        // Substring-matched against the forward-slash-normalized path.
        default: OptionDefault::StringList(&[
            // Next.js App Router routing files.
            "/page.",
            "/layout.",
            "/route.",
            "/error.",
            "/loading.",
            "/not-found.",
            "/default.",
            "/template.",
            "/global-error.",
            "/middleware.",
            "/instrumentation.",
            // Next.js metadata files (consumed by the framework, not user code).
            "/manifest.",
            "/robots.",
            "/sitemap.",
            "/icon.",
            "/apple-icon.",
            "/opengraph-image.",
            "/twitter-image.",
            "/favicon.",
            // Next.js Pages Router.
            "/_app.",
            "/_document.",
            "/_error.",
            // SvelteKit routing.
            "/+page.",
            "/+layout.",
            "/+server.",
            // Common project-config files.
            "/next.config.",
            "/vite.config.",
            "/vitest.config.",
            "/tsup.config.",
            "/tailwind.config.",
            "/postcss.config.",
            "/astro.config.",
            "/svelte.config.",
            "/remix.config.",
            "/playwright.config.",
            "/jest.config.",
            "/rollup.config.",
            "/webpack.config.",
            "/babel.config.",
            "/eslint.config.",
            "/prettier.config.",
        ]),
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
        // FinalizeContext doesn't carry per-check options today, so we
        // pull defaults straight from the static schema. cd-7h5 follow-up
        // can plumb options through once a user-facing override matters.
        let opts = OrphanOptions {
            include_type_only: false,
            test_patterns: string_list_default(&OE_OPTIONS[1].default),
            framework_entry_patterns: string_list_default(&OE_OPTIONS[2].default),
        };

        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());

        compute_orphans(&imports, &exports, &opts)
    }
}

fn string_list_default(d: &OptionDefault) -> Vec<String> {
    match d {
        OptionDefault::StringList(xs) => xs.iter().map(|s| (*s).to_string()).collect(),
        _ => Vec::new(),
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

fn compute_orphans(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    opts: &OrphanOptions,
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
