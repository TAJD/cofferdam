use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ImportRecord, InvariantsRuntime, LayersConfig, IMPORTS as GRAPH_IMPORTS,
    INVARIANTS as GRAPH_INVARIANTS, LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, Priority, Severity,
    SourceFile,
};

const IV_META: CheckMeta = CheckMeta {
    id: "Design.InvariantViolation",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation:
        "An import edge violates a `[invariants]` rule declared in cofferdam.invariants.toml.",
    body: include_str!("../../docs/Design.InvariantViolation.md"),
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
        let imports: Vec<ImportRecord> = ctx
            .corpus
            .with_slot(&GRAPH_IMPORTS, |slot| slot.records().cloned().collect());
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
        .map(|p| super::relative_normalised(project_root, p));
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
