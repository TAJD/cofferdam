use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use cofferdam_core::dsl::ast::TopPredicate;
use cofferdam_core::dsl::evaluator::{eval_predicate, eval_top, EvalCtx, GlobCache};
use cofferdam_core::dsl::parser::parse_predicate;
use cofferdam_core::graph::{
    ExportRecord, ImportRecord, InvariantsRuntime, LayersConfig, EXPORTS as GRAPH_EXPORTS,
    IMPORTS as GRAPH_IMPORTS, INVARIANTS as GRAPH_INVARIANTS, LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    Priority, ScriptedInvariantSpec, Severity, SourceFile, Span,
};

const SI_META: CheckMeta = CheckMeta {
    id: "Design.ScriptedInvariant",
    category: Category::Design,
    base_priority: 7,
    default_severity: Severity::Medium,
    explanation: "A scripted invariant declared in cofferdam.invariants.toml under [invariants.scripted] is violated for this file.",
    body: include_str!("../../docs/Design.ScriptedInvariant.md"),
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
        let glob_cache = GlobCache::new();

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
                glob_cache: &glob_cache,
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
