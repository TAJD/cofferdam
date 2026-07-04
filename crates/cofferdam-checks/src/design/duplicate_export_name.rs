use std::collections::BTreeMap;
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    Priority, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{Declaration, ExportNamedDeclaration, VariableDeclaration};
use oxc_ast_visit::Visit;

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
    body: include_str!("../../docs/Design.DuplicateExportName.md"),
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

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&EXPORTS, |slot, path| slot.retain(|e| e.file != path));
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
        let mut by_name: BTreeMap<String, Vec<NamedExport>> = BTreeMap::new();
        // Read-only (cd-32): a draining read would empty the slot as a
        // side effect of finalize, which is fine for a one-shot analyze
        // but corrupts `Engine::analyze_incremental`'s persistent
        // `AnalysisState` — the next incremental call would finalize
        // over an empty slot for every file that didn't just change.
        ctx.corpus.with_slot(&EXPORTS, |slot| {
            for exp in slot.iter().cloned() {
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
