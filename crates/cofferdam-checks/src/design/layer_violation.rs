use std::collections::HashMap;
use std::path::Path;

use cofferdam_core::graph::{
    ImportRecord, LayersConfig, IMPORTS as GRAPH_IMPORTS, LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, Priority, Severity,
    SourceFile,
};

const LV_META: CheckMeta = CheckMeta {
    id: "Design.LayerViolation",
    category: Category::Design,
    base_priority: 9,
    default_severity: Severity::High,
    explanation: "An import crosses a declared architectural layer in a direction not permitted by [layers].allow.",
    body: include_str!("../../docs/Design.LayerViolation.md"),
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
