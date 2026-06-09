use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
use crate::public_api::{resolve_public_api, PublicApi};
use cofferdam_core::graph::{
    ExportKind, ExportRecord, InvariantsRuntime, EXPORTS as GRAPH_EXPORTS,
    INVARIANTS as GRAPH_INVARIANTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, OptionDefault,
    OptionKind, OptionSpec, Priority, Severity, SourceFile,
};
use cofferdam_graph::{normalized_file_path, EdgeKind, Graph, Value, CANONICAL_GRAPH};
use smol_str::SmolStr;

#[derive(Debug, Clone)]
pub struct OrphanOptions {
    pub include_type_only: bool,
    pub test_patterns: Vec<String>,
    pub framework_entry_patterns: Vec<String>,
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
    body: include_str!("../../docs/Design.OrphanExport.md"),
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

/// Test-only entry point that builds an ephemeral canonical graph
/// from flat-table records before delegating to the production
/// graph-query path. Production code (`Design.OrphanExport::finalize`)
/// reads the engine-built `CANONICAL_GRAPH` slot directly and calls
/// [`compute_orphans_on_graph`].
#[cfg(test)]
pub fn compute_orphans(
    imports: &[cofferdam_core::graph::ImportRecord],
    exports: &[ExportRecord],
    opts: &OrphanOptions,
    public_api: &PublicApi,
) -> Vec<Issue> {
    #[cfg(test)]
    use cofferdam_graph::build_canonical_graph;
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
