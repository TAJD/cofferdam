//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::path::PathBuf;

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
    explanation: "Functions with too many parameters are hard to call correctly. Pass an options object instead.",
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
                severity: Severity::Warning,
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
    explanation: "The same name is exported from multiple files. Barrel re-exports collide silently and importers can't tell which one they got.",
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
                severity: Severity::Warning,
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
