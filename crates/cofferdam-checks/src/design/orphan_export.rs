use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
use cofferdam_core::graph::{
    ExportKind, ExportRecord, InvariantsRuntime, EXPORTS as GRAPH_EXPORTS,
    INVARIANTS as GRAPH_INVARIANTS,
};
use cofferdam_core::path_key;
use cofferdam_core::public_api::{resolve_public_api, PublicApi};
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
    pub test_imports_count: bool,
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
        name: "test_imports_count",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(true),
        doc: "Whether an import made from a test file (per `test_file_patterns`) counts as consumption. Default true — a test-only-consumed export is not an orphan. Set false to require a non-test consumer; exports imported only by test files are then reported with a distinct message instead of being silently treated as unconsumed.",
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
            test_imports_count: ctx.options.get_bool("test_imports_count").unwrap_or(true),
        };

        // cd-9hp.9 cp3 — graph-backed query path. The engine
        // populates CANONICAL_GRAPH after pass 1 finishes; OrphanExport
        // walks it instead of joining the flat IMPORTS / EXPORTS
        // tables itself. EXPORTS is still consumed here for the
        // per-export reporting payload (span, file, name) — the graph
        // doesn't yet carry export sites. DeadExport / ImportCycle /
        // LayerViolation will migrate behind this check.
        let exports: std::sync::Arc<Vec<ExportRecord>> =
            ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.to_vec());
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
#[derive(Default)]
struct ConsumptionSet {
    ns_touched: bool,
    default: bool,
    named: HashSet<SmolStr>,
}

/// Consumption tallied two ways: `any` counts every importer,
/// `non_test` counts only importers that don't match
/// `test_file_patterns`. Both are kept so the caller can (a) decide
/// whether an export is an orphan under the active `test_imports_count`
/// reading and (b) still tell "never imported" apart from "imported
/// only by test files" when reporting under the strict reading.
#[derive(Default)]
struct FileConsumption {
    any: ConsumptionSet,
    non_test: ConsumptionSet,
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
fn summarise_incoming(
    g: &Graph,
    file_node: cofferdam_graph::NodeId,
    test_patterns: &[String],
) -> FileConsumption {
    let mut out = FileConsumption::default();
    for (src, kind, attrs) in g.incoming(file_node) {
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
        // Importer is a test file when its node path (already
        // forward-slash-normalised by build_canonical_graph) matches
        // one of `test_file_patterns` — the same list that gates
        // exports declared in test files.
        let from_test_file = match g.node(src) {
            Some(cofferdam_graph::NodeKind::File { path, .. }) => {
                matches_substring(path, test_patterns)
            }
            _ => false,
        };

        let apply = |set: &mut ConsumptionSet| match import_kind {
            "namespace" => set.ns_touched = true,
            "default" => set.default = true,
            "named" => match &source_name {
                Some(name) if name.as_str() == "default" => set.default = true,
                Some(name) => {
                    set.named.insert(name.clone());
                }
                None => {}
            },
            "side_effect" => {}
            _ => {}
        };
        apply(&mut out.any);
        if !from_test_file {
            apply(&mut out.non_test);
        }
    }
    out
}

/// Whether `set` counts as consuming `kind`/`name`. Folds the
/// namespace-import shortcut (a namespace import claims every named
/// export) into the same boolean the caller checks — matches the
/// pre-cd-320 behaviour of treating a namespace-touched file as fully
/// consumed rather than special-casing it at the call site.
fn is_consumed(kind: &ExportKind, name: &str, set: &ConsumptionSet) -> bool {
    match kind {
        ExportKind::Default => set.default,
        ExportKind::Named => set.ns_touched || set.named.contains(&SmolStr::new(name)),
        ExportKind::ReExport => true,
    }
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
            Some(id) => summarise_incoming(g, id, &opts.test_patterns),
            // File never made it into the graph — happens only when
            // no record at all referenced it. Treat as zero
            // consumption (every named/default export is orphan).
            None => FileConsumption::default(),
        };

        for exp in sorted {
            // Re-exports are forwarding nodes, not orphan candidates.
            if matches!(exp.kind, ExportKind::ReExport) {
                continue;
            }
            if exp.type_only && !opts.include_type_only {
                continue;
            }

            // `test_imports_count` picks which bucket decides orphan
            // status. `consumed_any` is still needed under the strict
            // reading to tell "never imported" apart from "imported
            // only by test files" — a test-only consumer clears
            // `any` but not `non_test`.
            let consumed_any = is_consumed(&exp.kind, &exp.name, &consumption.any);
            let (consumed, test_only) = if opts.test_imports_count {
                (consumed_any, false)
            } else {
                let consumed_non_test = is_consumed(&exp.kind, &exp.name, &consumption.non_test);
                (consumed_non_test, !consumed_non_test && consumed_any)
            };

            if !consumed {
                let display_name = if matches!(exp.kind, ExportKind::Default) {
                    "default export".to_string()
                } else {
                    format!("`{}`", exp.name)
                };
                let message = if test_only {
                    format!("{} is imported only by test files", display_name)
                } else {
                    format!(
                        "{} is exported but never imported in the project",
                        display_name
                    )
                };
                issues.push(Issue {
                    check_id: OE_META.id.to_string(),
                    message,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ImportKind, ImportRecord, ImportedName};
    use cofferdam_core::Span;
    use std::path::PathBuf;

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        }
    }

    fn opts(test_imports_count: bool) -> OrphanOptions {
        OrphanOptions {
            include_type_only: false,
            test_patterns: vec![
                ".test.".to_string(),
                ".spec.".to_string(),
                "_test.".to_string(),
                "_spec.".to_string(),
                "/__tests__/".to_string(),
                "/__mocks__/".to_string(),
            ],
            framework_entry_patterns: Vec::new(),
            test_imports_count,
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
            span: span(),
        }
    }

    fn findings(
        exports: &[ExportRecord],
        imports: &[ImportRecord],
        test_imports_count: bool,
    ) -> Vec<Issue> {
        compute_orphans(
            imports,
            exports,
            &opts(test_imports_count),
            &PublicApi::default(),
        )
    }

    #[test]
    fn same_dir_test_file_import_counts_by_default() {
        let src = PathBuf::from("/proj/lib/account-capabilities.ts");
        let test = PathBuf::from("/proj/lib/account-capabilities.test.ts");
        let exports = vec![named_export(&src, "entitlementFor")];
        let imports = vec![named_import(&test, &src, "entitlementFor")];

        assert!(findings(&exports, &imports, true).is_empty());
    }

    #[test]
    fn same_dir_test_file_import_flagged_as_test_only_when_strict() {
        let src = PathBuf::from("/proj/lib/account-capabilities.ts");
        let test = PathBuf::from("/proj/lib/account-capabilities.test.ts");
        let exports = vec![named_export(&src, "entitlementFor")];
        let imports = vec![named_import(&test, &src, "entitlementFor")];

        let issues = findings(&exports, &imports, false);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].message,
            "`entitlementFor` is imported only by test files"
        );
    }

    #[test]
    fn tests_subdir_import_counts_by_default() {
        let src = PathBuf::from("/proj/lib/account-capabilities.ts");
        let test = PathBuf::from("/proj/lib/__tests__/account-capabilities.ts");
        let exports = vec![named_export(&src, "entitlementFor")];
        let imports = vec![named_import(&test, &src, "entitlementFor")];

        assert!(findings(&exports, &imports, true).is_empty());
    }

    #[test]
    fn tests_subdir_import_flagged_as_test_only_when_strict() {
        let src = PathBuf::from("/proj/lib/account-capabilities.ts");
        let test = PathBuf::from("/proj/lib/__tests__/account-capabilities.ts");
        let exports = vec![named_export(&src, "entitlementFor")];
        let imports = vec![named_import(&test, &src, "entitlementFor")];

        let issues = findings(&exports, &imports, false);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].message,
            "`entitlementFor` is imported only by test files"
        );
    }

    #[test]
    fn source_file_import_never_flagged() {
        let src = PathBuf::from("/proj/lib/account-capabilities.ts");
        let consumer = PathBuf::from("/proj/lib/worker.ts");
        let exports = vec![named_export(&src, "entitlementFor")];
        let imports = vec![named_import(&consumer, &src, "entitlementFor")];

        assert!(findings(&exports, &imports, true).is_empty());
        assert!(findings(&exports, &imports, false).is_empty());
    }

    #[test]
    fn no_importer_at_all_is_never_imported_in_both_modes() {
        let src = PathBuf::from("/proj/lib/dead.ts");
        let exports = vec![named_export(&src, "dead")];

        for test_imports_count in [true, false] {
            let issues = findings(&exports, &[], test_imports_count);
            assert_eq!(issues.len(), 1);
            assert_eq!(
                issues[0].message,
                "`dead` is exported but never imported in the project"
            );
        }
    }
}
