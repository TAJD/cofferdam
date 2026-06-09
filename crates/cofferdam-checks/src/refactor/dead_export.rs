use cofferdam_core::{
    path_key, Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, Priority,
    Severity, SourceFile,
};
use std::collections::HashMap;

// ─── Refactor.DeadExport ───────────────────────────────────────────────────
//
// Reads the project graph in `finalize`. An export is "dead" if it has at
// least one consumer (so it's not orphan), but every consumer imports the
// local binding and never references it after the import — typically left
// behind by a refactor that removed call sites without trimming the
// import + export.
//
// Local-use counts are computed by the engine's pass-1 graph builder
// (cofferdam-engine::graph::count_local_uses). Both value and type
// references are counted.

const DEX_META: CheckMeta = CheckMeta {
    id: "Refactor.DeadExport",
    category: Category::Refactor,
    base_priority: 4,
    default_severity: Severity::Low,
    explanation: "Every importer of this export imports its local binding and never references it. The export is dead even though it appears used.",
    body: include_str!("../../docs/Refactor.DeadExport.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

/// `Refactor.DeadExport` — finalize-stage cross-file check that
/// flags exports nothing imports. Differs from `Design.OrphanExport`
/// in that DeadExport excludes the project's declared public-API
/// surface; see `CheckMeta` for the precise distinction.
pub struct DeadExport;

impl Check for DeadExport {
    fn meta(&self) -> &'static CheckMeta {
        &DEX_META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        use cofferdam_core::graph::{
            ExportKind as GExportKind, ExportRecord as GExportRecord, ImportKind as GImportKind,
            ImportRecord as GImportRecord, EXPORTS as G_EXPORTS, IMPORTS as G_IMPORTS,
        };
        use std::collections::HashSet;

        let imports: Vec<GImportRecord> = ctx.corpus.with_slot(&G_IMPORTS, |slot| slot.clone());
        let exports: Vec<GExportRecord> = ctx.corpus.with_slot(&G_EXPORTS, |slot| slot.clone());

        // For each (target_path_key, source_name) collect: how many
        // consumers imported it, and how many of those consumers used it.
        let mut consumers: HashMap<(String, String), (u32, u32)> = HashMap::new();
        // Files where a namespace import lives — those touch every named
        // export and we can't tell which are unused without member-access
        // analysis. So a file with any namespace consumer is exempt from
        // dead-export.
        let mut namespace_touched: HashSet<String> = HashSet::new();
        // Files reached as re-export sources — also exempt, the
        // re-exporter's barrel is the consumer.
        let mut reexport_sources: HashSet<String> = HashSet::new();

        for imp in &imports {
            let Some(resolved) = &imp.resolved else {
                continue;
            };
            let key = path_key(resolved);
            for n in &imp.names {
                match n.kind {
                    GImportKind::Namespace => {
                        namespace_touched.insert(key.clone());
                    }
                    GImportKind::Default => {
                        let entry = consumers
                            .entry((key.clone(), "default".to_string()))
                            .or_insert((0, 0));
                        entry.0 += 1;
                        if n.local_use_count > 0 {
                            entry.1 += 1;
                        }
                    }
                    GImportKind::Named => {
                        let entry = consumers
                            .entry((key.clone(), n.source_name.clone()))
                            .or_insert((0, 0));
                        entry.0 += 1;
                        if n.local_use_count > 0 {
                            entry.1 += 1;
                        }
                    }
                }
            }
        }
        for exp in &exports {
            if let Some(src) = &exp.resolved_source {
                reexport_sources.insert(path_key(src));
            }
        }

        let mut issues = Vec::new();
        for exp in &exports {
            // Re-export forwarders aren't endpoints.
            if matches!(exp.kind, GExportKind::ReExport) {
                continue;
            }
            // Type-only is too noisy without proper type-aware analysis.
            if exp.type_only {
                continue;
            }
            let file_key = path_key(&exp.file);
            if namespace_touched.contains(&file_key) || reexport_sources.contains(&file_key) {
                continue;
            }
            let probe = match exp.kind {
                GExportKind::Default => (file_key.clone(), "default".to_string()),
                GExportKind::Named => (file_key.clone(), exp.name.clone()),
                GExportKind::ReExport => continue,
            };
            let Some(&(total, used)) = consumers.get(&probe) else {
                continue; // No consumer at all → that's OrphanExport, not us.
            };
            if total == 0 || used > 0 {
                continue;
            }
            // total ≥ 1 and zero consumers reference the binding. Dead.
            let display = if matches!(exp.kind, GExportKind::Default) {
                "default export".to_string()
            } else {
                format!("`{}`", exp.name)
            };
            issues.push(Issue {
                check_id: DEX_META.id.to_string(),
                message: format!(
                    "{} is imported by {} file(s) but never referenced after import",
                    display, total
                ),
                file: exp.file.clone(),
                location: Location::from_span(&exp.file, exp.span),
                priority: Priority(DEX_META.base_priority),
                severity: DEX_META.default_severity,
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
}
