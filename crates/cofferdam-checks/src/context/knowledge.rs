//! `Context.Knowledge` (CD-162) — curated `.cofferdam/knowledge/*.md`
//! notes matched against the `cofferdam context` `ChangeSet` via glob /
//! layer / predicate-DSL selectors (spec: wiki
//! `cofferdam-context-product-criteria`, section 3).
//!
//! This is the FILE half of curated knowledge only. Inline
//! `// @cofferdam-context:` annotations are CD-163/CP7 — out of scope
//! here.
//!
//! Load-time validation reuses the same "warn loudly, never silently
//! match nothing" policy as CD-150: a bad glob or predicate drops just
//! that selector (not the whole note) and produces a warning `Issue`
//! from `finalize()`. A note left with zero valid selectors after
//! validation also warns, since it would otherwise silently never fire.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use globset::Glob;

use cofferdam_core::changeset::ChangeSet;
use cofferdam_core::context_item::ContextItem;
use cofferdam_core::dsl::ast::{Comparison, Op, Operand, Predicate, Subject};
use cofferdam_core::dsl::evaluator::{eval_predicate, EvalCtx, GlobCache};
use cofferdam_core::dsl::parser::parse_predicate;
use cofferdam_core::graph::{
    ExportRecord, ImportRecord, InvariantsRuntime, LayersConfig, EXPORTS as GRAPH_EXPORTS,
    IMPORTS as GRAPH_IMPORTS, INVARIANTS as GRAPH_INVARIANTS, LAYERS as GRAPH_LAYERS,
};
use cofferdam_core::relevance;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Priority, RelatedSpan,
    Severity, SourceFile,
};

/// Cap on a knowledge note's body length (CD-204): without one, a
/// single `priority: high` note (pinned — never evicted by the digest
/// budget, see `context_digest::assemble`) can silently blow any
/// `--budget`. Truncated bodies get a visible `[truncated, ...]`
/// marker plus a load warning, per this module's "warn loudly, never
/// silently match nothing" policy.
const MAX_NOTE_BODY_CHARS: usize = 8_000;

const META: CheckMeta = CheckMeta {
    id: "Context.Knowledge",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Low,
    explanation: "Curated `.cofferdam/knowledge/*.md` notes matched against the current change via glob/layer/predicate selectors.",
    body: include_str!("../../docs/Context.Knowledge.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // run() is a no-op — all the work happens in finalize()/context_items(),
    // neither of which the per-file cache covers.
    pure_run: true,
};

/// `Context.Knowledge` — see module docs.
pub struct Knowledge;

impl Check for Knowledge {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    /// Loads knowledge notes purely to surface load-time validation
    /// warnings as `Issue`s (checks must never `println!`/`eprintln!`
    /// directly — findings, including advisory warnings, go through
    /// `Issue`). `context_items` reloads independently; see its doc
    /// comment for why the duplicate (cheap) load is the simpler design.
    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let root = resolve_project_root(ctx);
        let layers: Option<LayersConfig> = ctx.corpus.with_slot(&GRAPH_LAYERS, |s| s.clone());
        let load = load_knowledge_dir(&root, layers.as_ref());
        load.warnings
            .into_iter()
            .map(|(file, message)| Issue {
                check_id: META.id.to_string(),
                message,
                location: super::whole_file_location(&file),
                file,
                priority: Priority(META.base_priority),
                severity: META.default_severity,
                related: Vec::new(),
            })
            .collect()
    }

    /// Reloads knowledge notes (a handful of small markdown files — the
    /// I/O is negligible) rather than sharing state with `finalize` via
    /// a corpus slot. Keeps this check free of a `Send + Default`
    /// corpus-slot shape for `KnowledgeNote` and keeps `finalize`
    /// (warnings) and `context_items` (matching) independently
    /// testable.
    fn context_items(
        &self,
        changeset: &ChangeSet,
        ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        let root = resolve_project_root(ctx);
        let layers: Option<LayersConfig> = ctx.corpus.with_slot(&GRAPH_LAYERS, |s| s.clone());
        let load = load_knowledge_dir(&root, layers.as_ref());
        if load.notes.is_empty() {
            return Vec::new();
        }

        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |s| s.to_vec());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |s| s.to_vec());

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

        let glob_cache = GlobCache::new();
        let empty_imports: Vec<ImportRecord> = Vec::new();
        let empty_exports: Vec<ExportRecord> = Vec::new();

        let mut items = Vec::new();
        for note in &load.notes {
            let Some(pred) = &note.combined else {
                continue;
            };
            let mut matched: Vec<PathBuf> = Vec::new();
            for file in &changeset.files {
                let file_imports = imports_by_file.get(file).unwrap_or(&empty_imports);
                let file_exports = exports_by_file.get(file).unwrap_or(&empty_exports);
                let eval_ctx = EvalCtx {
                    file_path: file,
                    project_root: &root,
                    imports: file_imports,
                    exports: file_exports,
                    layers: layers.as_ref(),
                    glob_cache: &glob_cache,
                };
                if eval_predicate(pred, &eval_ctx).unwrap_or(false) {
                    matched.push(file.clone());
                }
            }
            if matched.is_empty() {
                continue;
            }
            let related: Vec<RelatedSpan> = matched
                .iter()
                .map(|f| RelatedSpan {
                    file: f.clone(),
                    location: super::whole_file_location(f),
                })
                .collect();
            items.push(ContextItem {
                check_id: META.id.to_string(),
                title: note.title.clone(),
                body: note.body.clone(),
                score: note.priority.score(),
                pinned: note.priority.pinned(),
                related,
                explain: Some(format!(
                    "{} ({}) matched {} changed file(s), e.g. {}",
                    note.source_path.display(),
                    note.selector_summary(),
                    matched.len(),
                    matched[0].display(),
                )),
            });
        }
        items
    }
}

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Project root for locating `.cofferdam/knowledge/`. Prefers the
/// already-resolved root the engine published for `Design.ScriptedInvariant`
/// / layer matching (`LAYERS`, then `INVARIANTS`) — same source of
/// truth, and avoids each check independently walking the filesystem.
/// Falls back to a `.git`-boundary walk from the process CWD only when
/// neither corpus slot is populated (no `cofferdam.toml` /
/// `cofferdam.invariants.toml` at all). Note: `std::env::current_dir`
/// mutation via `set_current_dir` races across parallel test threads
/// (see `crates/cofferdam-cli/src/doctor.rs`'s doc comment on the same
/// hazard) — tests should prefer the corpus-backed path over exercising
/// this fallback.
fn resolve_project_root(ctx: &FinalizeContext<'_>) -> PathBuf {
    if let Some(layers) = ctx.corpus.with_slot(&GRAPH_LAYERS, |s| s.clone()) {
        return layers.project_root;
    }
    if let Some(runtime) = ctx.corpus.with_slot(&GRAPH_INVARIANTS, |s| s.clone()) {
        let runtime: InvariantsRuntime = runtime;
        return runtime.project_root;
    }
    find_project_root(&current_dir())
}

/// Walk up from `start` to the nearest `.git` directory (the repo
/// root), mirroring `cofferdam.toml`'s own discovery boundary
/// (`crates/cofferdam-engine/src/config/loader.rs`). Falls back to
/// `start` itself when no `.git` is found — keeps knowledge loading a
/// no-op (empty `LoadResult`) instead of erroring in a non-git
/// checkout.
pub fn find_project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

pub fn knowledge_dir_path(project_root: &Path) -> PathBuf {
    project_root.join(".cofferdam").join("knowledge")
}

/// `high | normal | low` — maps to `ContextItem.score`/`pinned` per the
/// product spec ("high-priority knowledge notes can never be evicted by
/// precedent/blast-radius filler").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotePriority {
    High,
    Normal,
    Low,
}

impl NotePriority {
    fn score(self) -> i32 {
        match self {
            NotePriority::High => relevance::VERIFIED,
            NotePriority::Normal => relevance::DIRECT,
            NotePriority::Low => relevance::INFERRED,
        }
    }

    fn pinned(self) -> bool {
        matches!(self, NotePriority::High)
    }
}

/// One loaded, validated `.cofferdam/knowledge/*.md` note.
#[derive(Debug, Clone)]
pub struct KnowledgeNote {
    pub source_path: PathBuf,
    pub title: String,
    pub body: String,
    pub priority: NotePriority,
    /// Glob patterns that compiled successfully.
    pub raw_paths: Vec<String>,
    pub raw_layers: Vec<String>,
    pub raw_predicate: Option<String>,
    /// `(paths[0] matches) or (paths[1] matches) or (layers[0] in) or
    /// ... or predicate` — `None` when every selector failed
    /// validation (or none were declared), which `finalize` already
    /// warned about.
    combined: Option<Predicate>,
}

impl KnowledgeNote {
    fn selector_summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.raw_paths.is_empty() {
            parts.push(format!("paths={:?}", self.raw_paths));
        }
        if !self.raw_layers.is_empty() {
            parts.push(format!("layers={:?}", self.raw_layers));
        }
        if let Some(p) = &self.raw_predicate {
            parts.push(format!("predicate=`{p}`"));
        }
        if parts.is_empty() {
            "no selectors".to_string()
        } else {
            parts.join(", ")
        }
    }
}

pub struct LoadResult {
    pub notes: Vec<KnowledgeNote>,
    /// `(source note path, message)` — the path is always the specific
    /// note file the warning is about (every warning today originates
    /// inside the per-note `load_note` pass; a missing/unreadable
    /// knowledge directory returns an empty `LoadResult` with no
    /// warning at all, so there is currently no genuinely
    /// directory-scoped warning to attribute to `dir` instead).
    pub warnings: Vec<(PathBuf, String)>,
}

/// Load and validate every `.cofferdam/knowledge/*.md` file under
/// `project_root`. A missing (or unreadable) knowledge directory is
/// not an error — projects without curated knowledge get an empty
/// result, per the product spec's zero-config bar. Sorted by filename
/// for deterministic digest ordering. `layers` is the project's
/// configured layers (when available) — used to catch `match.layers`
/// selectors that name an undeclared layer (CD-207); pass `None` when
/// no layers config is available and that validation is skipped.
pub fn load_knowledge_dir(project_root: &Path, layers: Option<&LayersConfig>) -> LoadResult {
    let dir = knowledge_dir_path(project_root);
    let mut result = LoadResult {
        notes: Vec::new(),
        warnings: Vec::new(),
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();

    for path in paths {
        match load_note(&path, layers) {
            Ok((note, warnings)) => {
                result
                    .warnings
                    .extend(warnings.into_iter().map(|msg| (path.clone(), msg)));
                if let Some(n) = note {
                    result.notes.push(n);
                }
            }
            Err(msg) => result.warnings.push((path.clone(), msg)),
        }
    }
    result
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct MatchSpec {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    layers: Vec<String>,
    #[serde(default)]
    predicate: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Frontmatter {
    title: String,
    #[serde(default, rename = "match")]
    match_spec: MatchSpec,
    #[serde(default)]
    priority: Option<String>,
}

/// Loads and validates one note. `Err` is reserved for failures that
/// mean there is no usable note at all (unreadable file, no
/// frontmatter block, unparseable YAML, missing `title`); everything
/// else degrades to a `Some(note)` plus warnings so one bad selector
/// doesn't take out the whole note.
fn load_note(
    path: &Path,
    layers: Option<&LayersConfig>,
) -> Result<(Option<KnowledgeNote>, Vec<String>), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: failed to read: {e}", path.display()))?;
    let Some((yaml, body)) = split_frontmatter(&text) else {
        return Err(format!(
            "{}: missing YAML frontmatter (expected a `---` delimited block at the top of the file)",
            path.display()
        ));
    };
    let fm: Frontmatter = serde_yaml::from_str(yaml)
        .map_err(|e| format!("{}: invalid frontmatter: {e}", path.display()))?;

    let mut warnings = Vec::new();

    let mut valid_paths = Vec::new();
    for pattern in &fm.match_spec.paths {
        match Glob::new(pattern) {
            Ok(_) => valid_paths.push(pattern.clone()),
            Err(e) => warnings.push(format!(
                "{}: invalid glob `{pattern}` in match.paths, ignoring this selector: {e}",
                path.display()
            )),
        }
    }

    // Validate `match.layers` names against the project's declared
    // layers (CD-207): an undeclared/typo'd layer name must not be
    // allowed to silently produce a selector that matches nothing,
    // per this module's "warn loudly, never silently match nothing"
    // policy. Skipped (all names pass through unchanged) when no
    // layers config is available to validate against.
    let valid_layers: Vec<String> = match layers {
        Some(layers_cfg) => {
            let mut valid = Vec::new();
            for layer_name in &fm.match_spec.layers {
                if layers_cfg.layers.contains_key(layer_name) {
                    valid.push(layer_name.clone());
                } else {
                    warnings.push(format!(
                        "{}: undeclared layer `{layer_name}` in match.layers, ignoring this selector: not found in the configured layers",
                        path.display()
                    ));
                }
            }
            valid
        }
        None => fm.match_spec.layers.clone(),
    };

    let predicate = match &fm.match_spec.predicate {
        Some(src) => match parse_predicate(src) {
            Ok(p) => Some(p),
            Err(e) => {
                warnings.push(format!(
                    "{}: invalid predicate `{src}` in match.predicate, ignoring this selector: {e}",
                    path.display()
                ));
                None
            }
        },
        None => None,
    };

    let priority = match fm.priority.as_deref() {
        None => NotePriority::Normal,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "high" => NotePriority::High,
            "normal" => NotePriority::Normal,
            "low" => NotePriority::Low,
            other => {
                warnings.push(format!(
                    "{}: unknown priority `{other}` (expected high|normal|low), defaulting to normal",
                    path.display()
                ));
                NotePriority::Normal
            }
        },
    };

    let combined = combined_predicate(&valid_paths, &valid_layers, predicate);
    if combined.is_none() {
        warnings.push(format!(
            "{}: note `{}` has no valid selectors after validation and will never fire",
            path.display(),
            fm.title
        ));
    }

    let trimmed_body = body.trim();
    let char_count = trimmed_body.chars().count();
    let note_body = if char_count > MAX_NOTE_BODY_CHARS {
        warnings.push(format!(
            "{}: note body is {char_count} chars, exceeds the {}k char limit and will be truncated",
            path.display(),
            MAX_NOTE_BODY_CHARS / 1000
        ));
        truncate_body(trimmed_body)
    } else {
        trimmed_body.to_string()
    };

    let note = KnowledgeNote {
        source_path: path.to_path_buf(),
        title: fm.title,
        body: note_body,
        priority,
        raw_paths: fm.match_spec.paths,
        raw_layers: fm.match_spec.layers,
        raw_predicate: fm.match_spec.predicate,
        combined,
    };
    Ok((Some(note), warnings))
}

/// `paths[0] matches or paths[1] matches or ... or layers[0] in or ...
/// or predicate` — reuses the predicate DSL's public `eval_predicate`
/// so path-glob and layer matching get the DSL's own semantics
/// (project-root relativization, Windows separator normalization) for
/// free instead of a second bespoke matcher.
fn combined_predicate(
    valid_paths: &[String],
    layers: &[String],
    predicate: Option<Predicate>,
) -> Option<Predicate> {
    let mut parts: Vec<Predicate> = Vec::new();
    for pattern in valid_paths {
        parts.push(Predicate::Comparison(Comparison {
            subject: Subject::Identifier("file".to_string()),
            op: Op::Matches,
            operand: Operand::String(pattern.clone()),
        }));
    }
    for layer in layers {
        parts.push(Predicate::Comparison(Comparison {
            subject: Subject::Identifier("file".to_string()),
            op: Op::In,
            operand: Operand::String(layer.clone()),
        }));
    }
    if let Some(p) = predicate {
        parts.push(p);
    }
    let mut iter = parts.into_iter();
    let first = iter.next()?;
    Some(iter.fold(first, |acc, p| Predicate::Or(Box::new(acc), Box::new(p))))
}

/// Truncates a note body to `MAX_NOTE_BODY_CHARS`, appending a visible
/// marker so truncation is never silent (the digest budget already
/// discloses omissions elsewhere; this is the equivalent for a single
/// oversized note).
fn truncate_body(body: &str) -> String {
    let char_count = body.chars().count();
    let kept: String = body.chars().take(MAX_NOTE_BODY_CHARS).collect();
    let over = char_count - MAX_NOTE_BODY_CHARS;
    format!(
        "{kept}\n\n[truncated, {over} chars over {}k char limit]",
        MAX_NOTE_BODY_CHARS / 1000
    )
}

/// Splits a leading `---\n ... \n---\n` YAML block from the markdown
/// body that follows. Returns `None` when the file doesn't open with
/// the frontmatter delimiter.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            let yaml = &rest[..idx];
            let body = &rest[idx + line.len()..];
            return Some((yaml, body));
        }
        idx += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::CorpusIndex;
    use tempfile::tempdir;

    fn write_note(dir: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn split_frontmatter_finds_yaml_and_body() {
        let text = "---\ntitle: X\n---\nBody text\n";
        let (yaml, body) = split_frontmatter(text).expect("frontmatter");
        assert_eq!(yaml, "title: X\n");
        assert_eq!(body, "Body text\n");
    }

    #[test]
    fn split_frontmatter_none_without_leading_delimiter() {
        assert!(split_frontmatter("no frontmatter here\n").is_none());
    }

    #[test]
    fn load_note_parses_full_example_from_spec() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "billing.md",
            r#"---
title: Billing invariants
match:
  paths: ["src/billing/**"]
  layers: ["billing"]
  predicate: "imports 'src/db'"
priority: high
---
Billing code must never round intermediate values.
"#,
        );
        let load = load_knowledge_dir(td.path(), None);
        assert!(load.warnings.is_empty(), "{:?}", load.warnings);
        assert_eq!(load.notes.len(), 1);
        let note = &load.notes[0];
        assert_eq!(note.title, "Billing invariants");
        assert_eq!(note.priority, NotePriority::High);
        assert!(note.priority.pinned());
        assert!(note.body.starts_with("Billing code"));
        assert!(note.combined.is_some());
    }

    #[test]
    fn missing_knowledge_dir_yields_empty_result() {
        let td = tempdir().unwrap();
        let load = load_knowledge_dir(td.path(), None);
        assert!(load.notes.is_empty());
        assert!(load.warnings.is_empty());
    }

    #[test]
    fn invalid_glob_warns_and_drops_only_that_selector() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "bad_glob.md",
            r#"---
title: Bad glob note
match:
  paths: ["src/[unterminated"]
  layers: ["ui"]
---
Body.
"#,
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes.len(), 1);
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("invalid glob")));
        // The layers selector still compiled, so the note isn't orphaned.
        assert!(load.notes[0].combined.is_some());
    }

    #[test]
    fn invalid_predicate_warns_and_drops_only_that_selector() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "bad_predicate.md",
            r#"---
title: Bad predicate note
match:
  paths: ["src/**"]
  predicate: "this is not a predicate((("
---
Body.
"#,
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes.len(), 1);
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("invalid predicate")));
        assert!(load.notes[0].combined.is_some());
    }

    #[test]
    fn note_with_no_selectors_warns_never_fires() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "no_selectors.md",
            "---\ntitle: Orphan\n---\nBody.\n",
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes.len(), 1);
        assert!(load.notes[0].combined.is_none());
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("has no valid selectors")));
    }

    #[test]
    fn missing_frontmatter_is_a_hard_load_error() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(&knowledge, "plain.md", "just markdown, no frontmatter\n");
        let load = load_knowledge_dir(td.path(), None);
        assert!(load.notes.is_empty());
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("missing YAML frontmatter")));
    }

    #[test]
    fn unknown_priority_defaults_to_normal_with_warning() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "weird_priority.md",
            "---\ntitle: X\npriority: extreme\nmatch:\n  paths: [\"src/**\"]\n---\nBody.\n",
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes[0].priority, NotePriority::Normal);
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("unknown priority")));
    }

    #[test]
    fn oversized_note_body_is_truncated_with_visible_marker_and_warns() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        let huge_body = "x".repeat(MAX_NOTE_BODY_CHARS + 500);
        write_note(
            &knowledge,
            "huge.md",
            &format!("---\ntitle: Huge\nmatch:\n  paths: [\"src/**\"]\n---\n{huge_body}\n"),
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes.len(), 1);
        let note = &load.notes[0];
        assert!(note
            .body
            .contains("[truncated, 500 chars over 8k char limit]"));
        assert_eq!(
            note.body.chars().count(),
            MAX_NOTE_BODY_CHARS
                + "\n\n[truncated, 500 chars over 8k char limit]"
                    .chars()
                    .count()
        );
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("exceeds the 8k char limit")));
    }

    #[test]
    fn note_body_within_limit_is_not_truncated() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "small.md",
            "---\ntitle: Small\nmatch:\n  paths: [\"src/**\"]\n---\nShort body.\n",
        );
        let load = load_knowledge_dir(td.path(), None);
        assert_eq!(load.notes[0].body, "Short body.");
        assert!(!load.warnings.iter().any(|(_, w)| w.contains("truncat")));
    }

    #[test]
    fn find_project_root_stops_at_git() {
        let td = tempdir().unwrap();
        std::fs::create_dir_all(td.path().join(".git")).unwrap();
        let nested = td.path().join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_project_root(&nested), td.path());
    }

    #[test]
    fn find_project_root_falls_back_to_start_without_git() {
        let td = tempdir().unwrap();
        let nested = td.path().join("no_git_here");
        std::fs::create_dir_all(&nested).unwrap();
        // No .git anywhere above `nested` inside the tempdir; the walk
        // continues past filesystem root and falls back to `start`.
        assert_eq!(find_project_root(&nested), nested);
    }

    #[test]
    fn undeclared_layer_warns_and_note_never_fires() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "typo_layer.md",
            "---\ntitle: Typo layer note\nmatch:\n  layers: [\"biling\"]\n---\nBody.\n",
        );
        let mut declared = std::collections::BTreeMap::new();
        declared.insert("billing".to_string(), vec!["src/billing/**".to_string()]);
        let layers = LayersConfig {
            project_root: td.path().to_path_buf(),
            layers: declared,
            allow: std::collections::BTreeMap::new(),
        };
        let load = load_knowledge_dir(td.path(), Some(&layers));
        assert_eq!(load.notes.len(), 1);
        // The only selector was the undeclared layer name, so the note
        // is left with no valid selector and would otherwise silently
        // never fire — must warn per the CD-150 policy.
        assert!(load.notes[0].combined.is_none());
        assert!(load
            .warnings
            .iter()
            .any(|(_, w)| w.contains("undeclared layer")));
    }

    #[test]
    fn declared_layer_passes_validation_without_warning() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "billing.md",
            "---\ntitle: Billing note\nmatch:\n  layers: [\"billing\"]\n---\nBody.\n",
        );
        let mut declared = std::collections::BTreeMap::new();
        declared.insert("billing".to_string(), vec!["src/billing/**".to_string()]);
        let layers = LayersConfig {
            project_root: td.path().to_path_buf(),
            layers: declared,
            allow: std::collections::BTreeMap::new(),
        };
        let load = load_knowledge_dir(td.path(), Some(&layers));
        assert_eq!(load.notes.len(), 1);
        assert!(load.notes[0].combined.is_some());
        assert!(load.warnings.is_empty(), "{:?}", load.warnings);
    }

    #[test]
    fn finalize_issue_uses_the_specific_note_path_not_the_knowledge_dir() {
        let td = tempdir().unwrap();
        let knowledge = knowledge_dir_path(td.path());
        write_note(
            &knowledge,
            "broken.md",
            "---\ntitle: Broken glob note\nmatch:\n  paths: [\"src/[unterminated\"]\n---\nBody.\n",
        );
        let layers = LayersConfig {
            project_root: td.path().to_path_buf(),
            layers: std::collections::BTreeMap::new(),
            allow: std::collections::BTreeMap::new(),
        };
        let corpus = CorpusIndex::default();
        corpus.with_slot(&GRAPH_LAYERS, |s| *s = Some(layers));
        let mut ctx = FinalizeContext::new(&corpus);

        let issues = Knowledge.finalize(&mut ctx);
        // The invalid glob leaves the note with no valid selector at
        // all, so both the "invalid glob" and the "no valid selectors"
        // warnings fire — both must point at the note file, not the dir.
        assert_eq!(issues.len(), 2);
        for issue in &issues {
            assert_eq!(issue.file, knowledge.join("broken.md"));
            assert_ne!(issue.file, knowledge, "must not be the directory");
        }
    }
}
