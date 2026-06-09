use cofferdam_core::graph::{InvariantsRuntime, INVARIANTS as GRAPH_INVARIANTS};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile, Span,
};

const BF_META: CheckMeta = CheckMeta {
    id: "Design.BoundaryFrozen",
    category: Category::Design,
    base_priority: 0,
    default_severity: Severity::Low,
    explanation: "File lives inside an architectural boundary marked frozen=true in cofferdam.invariants.toml. New code in this area should be reviewed against the boundary's stated reason.",
    body: include_str!("../../docs/Design.BoundaryFrozen.md"),
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
        let normalized_rel = super::relative_normalised(&runtime.project_root, &file.path);
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
