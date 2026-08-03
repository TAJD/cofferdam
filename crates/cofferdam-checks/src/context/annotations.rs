//! `Context.Annotations` (CD-163, CP7) — advisory `cofferdam context`
//! provider for inline `// @cofferdam-context: <text>` and block-comment
//! `/* @cofferdam-context: <text> */` annotations.
//!
//! Scope is the enclosing declaration: the innermost function/class whose
//! span contains the annotation (annotation written inside a body), or
//! else the nearest following function/class declaration (annotation
//! written directly above it, JSDoc-style). An annotation that matches
//! neither scopes to the whole module.
//!
//! Fires when the `ChangeSet` touches the annotated scope's line range
//! directly, or when it touches a file that directly imports the
//! annotated file (one hop only — unlike `Context.BlastRadius`'s bounded
//! multi-hop BFS, CD-163 only requires the direct-importer case).

use std::path::{Path, PathBuf};

use cofferdam_core::graph::{ImportRecord, IMPORTS as GRAPH_IMPORTS};
use cofferdam_core::{
    path_key, span_from_bytes, Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem,
    CorpusKey, FinalizeContext, Issue, Location, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{Class, Function};
use oxc_ast_visit::Visit;

/// Score for an annotation whose own scope is directly touched by the
/// changeset. Below `Context.BlastRadius`'s direct-caller score (100) and
/// direct-importer score (70) — an author-written annotation is a strong
/// signal but this provider can't verify it's still accurate the way a
/// graph-derived relation can.
const SCORE_DIRECT: i32 = 65;
/// Score when the annotation fires because a direct importer of its file
/// was changed rather than the annotated scope itself.
const SCORE_VIA_IMPORTER: i32 = 40;

static ANNOTATIONS: CorpusKey<Vec<AnnotationRecord>> =
    CorpusKey::new("Context.Annotations.records");

const META: CheckMeta = CheckMeta {
    id: "Context.Annotations",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "Advisory: surfaces author-written `// @cofferdam-context: ...` annotations \
        whose enclosing function/class (or whole module) was touched by the change, or whose \
        file was imported directly by a changed file.",
    // See `Context.Precedent`'s META.body comment for why this is a plain
    // string literal rather than `include_str!` — Context providers are
    // excluded from the gen-docs catalog.
    body: "Scans changed and unchanged files for `// @cofferdam-context: <text>` line comments \
        and `/* @cofferdam-context: <text> */` block comments. Each annotation is attributed to \
        its enclosing function/class declaration (or the whole module, if none). Fires when the \
        ChangeSet's line ranges overlap the annotated scope directly, or when the ChangeSet \
        touches a file that directly imports the annotated file. Never fires on unrelated edits.",
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes per-file AnnotationRecords into the corpus during run();
    // skipping run() on a cache hit would drop that file's annotations
    // and silently under-report at context_items() time, mirroring every
    // other corpus-writing check in this crate (see Context.Precedent).
    pure_run: false,
};

#[derive(Clone)]
struct AnnotationRecord {
    file: PathBuf,
    text: String,
    scope_name: Option<String>,
    /// 1-based inclusive [start, end] line range of the enclosing scope
    /// (or the whole file, for module scope).
    start_line: u32,
    end_line: u32,
    scope_span: Span,
}

/// `Context.Annotations` — CD-156/CD-163. See module docs.
pub struct Annotations;

impl Check for Annotations {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let hits = find_annotation_comments(&file.text);
        if hits.is_empty() {
            return Vec::new();
        }

        let mut collector = ScopeCollector { scopes: Vec::new() };
        collector.visit_program(parsed.program);

        let module_span = span_from_bytes(&file.text, 0, file.text.len() as u32);
        let module_end_line =
            span_from_bytes(&file.text, file.text.len() as u32, file.text.len() as u32).line;

        let records: Vec<AnnotationRecord> = hits
            .into_iter()
            .map(|(offset, text)| {
                let scope = collector.enclosing(offset);
                let (name, start_line, end_line, scope_span) = match scope {
                    Some(s) => (
                        Some(s.name),
                        span_from_bytes(&file.text, s.start, s.start).line,
                        span_from_bytes(&file.text, s.end, s.end).line,
                        span_from_bytes(&file.text, s.start, s.end),
                    ),
                    None => (None, 1, module_end_line, module_span),
                };
                AnnotationRecord {
                    file: file.path.clone(),
                    text,
                    scope_name: name,
                    start_line,
                    end_line,
                    scope_span,
                }
            })
            .collect();

        ctx.corpus
            .with_slot(&ANNOTATIONS, |slot| slot.extend(records));
        Vec::new()
    }

    fn context_items(
        &self,
        changeset: &ChangeSet,
        ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        let records: Vec<AnnotationRecord> =
            ctx.corpus.with_slot(&ANNOTATIONS, |slot| slot.clone());
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        compute_annotation_items(&records, &imports, changeset)
    }
}

/// One function/class declaration span discovered in the file, with its
/// display name.
struct DeclScope {
    name: String,
    start: u32,
    end: u32,
}

struct ScopeCollector {
    scopes: Vec<DeclScope>,
}

impl ScopeCollector {
    /// The enclosing declaration for a byte offset: the innermost scope
    /// whose span contains `offset` (annotation inside a body), else the
    /// nearest following scope (annotation written directly above a
    /// declaration), else `None` (module scope). Returns an owned copy
    /// so the caller isn't left holding a borrow of `self`.
    fn enclosing(&self, offset: u32) -> Option<ResolvedScope> {
        let containing = self
            .scopes
            .iter()
            .filter(|s| s.start <= offset && offset <= s.end)
            .min_by_key(|s| s.end - s.start);
        containing
            .or_else(|| {
                self.scopes
                    .iter()
                    .filter(|s| s.start > offset)
                    .min_by_key(|s| s.start)
            })
            .map(|s| ResolvedScope {
                name: s.name.clone(),
                start: s.start,
                end: s.end,
            })
    }
}

struct ResolvedScope {
    name: String,
    start: u32,
    end: u32,
}

impl<'a> Visit<'a> for ScopeCollector {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous function".to_string());
        self.scopes.push(DeclScope {
            name,
            start: node.span.start,
            end: node.span.end,
        });
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_class(&mut self, node: &Class<'a>) {
        let name = node
            .id
            .as_ref()
            .map(|id| id.name.as_str().to_string())
            .unwrap_or_else(|| "anonymous class".to_string());
        self.scopes.push(DeclScope {
            name,
            start: node.span.start,
            end: node.span.end,
        });
        oxc_ast_visit::walk::walk_class(self, node);
    }
}

/// Recognizes `// @cofferdam-context: <text>` and
/// `/* @cofferdam-context: <text> */` (single- or multi-line). Returns
/// each match's byte offset (of the comment's opening marker) and the
/// annotation text with the marker stripped and whitespace trimmed.
///
/// Raw-text scanning, not AST comment nodes — mirrors
/// `Consistency.BroadSuppression`/`Consistency.UnusedSuppression`'s
/// existing precedent for suppression-comment parsing (see
/// `crates/cofferdam-checks/src/consistency.rs`); oxc trivia isn't wired
/// up anywhere else in this codebase.
fn find_annotation_comments(text: &str) -> Vec<(u32, String)> {
    const MARKER: &str = "@cofferdam-context:";
    let mut out = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel) = text[search_from..].find(MARKER) {
        let marker_start = search_from + rel;
        let before = &text[..marker_start];

        // Find the nearest comment opener before the marker on the same
        // line (or, for block comments, anywhere before it) with no
        // closer in between.
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_prefix = &text[line_start..marker_start];

        let comment_start = if let Some(idx) = line_prefix.rfind("//") {
            Some(line_start + idx)
        } else if let Some(idx) = before.rfind("/*") {
            // Only valid if there's no intervening `*/` between the
            // opener and the marker (i.e. we're still inside that block
            // comment).
            let between = &text[idx..marker_start];
            if between.contains("*/") {
                None
            } else {
                Some(idx)
            }
        } else {
            None
        };

        if let Some(start) = comment_start {
            let after_marker = &text[marker_start + MARKER.len()..];
            let (raw_text, consumed_to) = if text[start..].starts_with("/*") {
                match after_marker.find("*/") {
                    Some(end_rel) => (
                        &after_marker[..end_rel],
                        marker_start + MARKER.len() + end_rel + 2,
                    ),
                    None => (after_marker, text.len()),
                }
            } else {
                match after_marker.find('\n') {
                    Some(end_rel) => (
                        &after_marker[..end_rel],
                        marker_start + MARKER.len() + end_rel,
                    ),
                    None => (after_marker, text.len()),
                }
            };
            let cleaned = raw_text
                .trim()
                .trim_end_matches("*/")
                .trim_end()
                .replace("\r\n", "\n");
            let cleaned = cleaned
                .lines()
                .map(|l| l.trim().trim_start_matches('*').trim())
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            if !cleaned.is_empty() {
                out.push((start as u32, cleaned));
            }
            search_from = consumed_to.max(marker_start + MARKER.len());
        } else {
            search_from = marker_start + MARKER.len();
        }
    }

    out
}

fn line_range_overlaps(changeset: &ChangeSet, file: &Path, start_line: u32, end_line: u32) -> bool {
    if !changeset.contains(file) {
        return false;
    }
    match changeset.line_ranges.get(file) {
        None => true,
        Some(ranges) if ranges.is_empty() => true,
        Some(ranges) => ranges
            .iter()
            .any(|r| r.start <= end_line && r.end >= start_line),
    }
}

#[cfg(test)]
fn zero_span() -> Span {
    Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    }
}

fn compute_annotation_items(
    records: &[AnnotationRecord],
    imports: &[ImportRecord],
    changeset: &ChangeSet,
) -> Vec<ContextItem> {
    if changeset.is_empty() {
        return Vec::new();
    }

    let mut items: Vec<ContextItem> = records
        .iter()
        .filter_map(|rec| {
            let scope_label = rec
                .scope_name
                .clone()
                .unwrap_or_else(|| "the module".to_string());

            if line_range_overlaps(changeset, &rec.file, rec.start_line, rec.end_line) {
                let title = format!("Annotation: {} ({})", rec.file.display(), scope_label);
                let explain = format!(
                    "annotation on {scope_label} in {} — scope directly touched by the change",
                    rec.file.display()
                );
                return Some(build_item(rec, title, explain, SCORE_DIRECT));
            }

            let rec_key = path_key(&rec.file);
            let mut importers: Vec<&Path> = imports
                .iter()
                .filter(|imp| {
                    imp.resolved
                        .as_deref()
                        .is_some_and(|r| path_key(r) == rec_key)
                })
                .map(|imp| imp.from_file.as_path())
                .filter(|f| changeset.contains(f))
                .collect();
            importers.sort();
            importers.dedup();

            let importer = importers.first()?;
            let title = format!("Annotation: {} ({})", rec.file.display(), scope_label);
            let explain = format!(
                "{} imports {} directly — annotation on {scope_label}",
                importer.display(),
                rec.file.display()
            );
            Some(build_item(rec, title, explain, SCORE_VIA_IMPORTER))
        })
        .collect();

    items.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    items
}

fn build_item(rec: &AnnotationRecord, title: String, explain: String, score: i32) -> ContextItem {
    ContextItem {
        check_id: META.id.to_string(),
        title,
        body: rec.text.clone(),
        score,
        pinned: false,
        related: vec![RelatedSpan {
            location: Location::from_span(&rec.file, rec.scope_span),
            file: rec.file.clone(),
        }],
        explain: Some(explain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ImportKind, ImportedName};
    use cofferdam_core::LineRange;

    fn named_import(from: &Path, resolved: &Path) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./m".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: "thing".to_string(),
                local_name: "thing".to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: zero_span(),
        }
    }

    fn rec(
        file: &Path,
        scope_name: Option<&str>,
        start_line: u32,
        end_line: u32,
    ) -> AnnotationRecord {
        AnnotationRecord {
            file: file.to_path_buf(),
            text: "handle with care".to_string(),
            scope_name: scope_name.map(|s| s.to_string()),
            start_line,
            end_line,
            scope_span: zero_span(),
        }
    }

    #[test]
    fn line_comment_annotation_is_extracted() {
        let src =
            "function f() {\n  // @cofferdam-context: do not remove the retry\n  doThing();\n}\n";
        let hits = find_annotation_comments(src);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "do not remove the retry");
    }

    #[test]
    fn block_comment_annotation_is_extracted() {
        let src = "/* @cofferdam-context: spans\n * multiple lines\n */\nfunction f() {}\n";
        let hits = find_annotation_comments(src);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "spans multiple lines");
    }

    #[test]
    fn no_marker_yields_no_hits() {
        assert!(find_annotation_comments("function f() { return 1; }").is_empty());
    }

    #[test]
    fn fires_on_edit_in_annotated_scope() {
        let file = PathBuf::from("/proj/lib.ts");
        let records = vec![rec(&file, Some("doThing"), 3, 8)];
        let mut changeset = ChangeSet::from_files([file.clone()]);
        changeset
            .line_ranges
            .insert(file.clone(), vec![LineRange { start: 4, end: 4 }]);

        let items = compute_annotation_items(&records, &[], &changeset);
        assert_eq!(items.len(), 1);
        assert!(items[0]
            .explain
            .as_deref()
            .unwrap()
            .contains("directly touched"));
    }

    #[test]
    fn fires_on_edit_of_direct_importer() {
        let lib = PathBuf::from("/proj/lib.ts");
        let caller = PathBuf::from("/proj/caller.ts");
        let records = vec![rec(&lib, Some("doThing"), 3, 8)];
        let imports = vec![named_import(&caller, &lib)];
        let changeset = ChangeSet::from_files([caller.clone()]);

        let items = compute_annotation_items(&records, &imports, &changeset);
        assert_eq!(items.len(), 1);
        assert!(items[0]
            .explain
            .as_deref()
            .unwrap()
            .contains("caller.ts imports"));
    }

    #[test]
    fn does_not_fire_on_unrelated_edit() {
        let lib = PathBuf::from("/proj/lib.ts");
        let unrelated = PathBuf::from("/proj/unrelated.ts");
        let records = vec![rec(&lib, Some("doThing"), 3, 8)];
        let changeset = ChangeSet::from_files([unrelated]);

        let items = compute_annotation_items(&records, &[], &changeset);
        assert!(items.is_empty());
    }

    #[test]
    fn does_not_fire_on_edit_outside_scope_in_same_file() {
        let file = PathBuf::from("/proj/lib.ts");
        let records = vec![rec(&file, Some("doThing"), 10, 20)];
        let mut changeset = ChangeSet::from_files([file.clone()]);
        changeset
            .line_ranges
            .insert(file.clone(), vec![LineRange { start: 1, end: 2 }]);

        let items = compute_annotation_items(&records, &[], &changeset);
        assert!(items.is_empty());
    }

    #[test]
    fn module_scope_annotation_fires_on_any_edit_in_file() {
        let file = PathBuf::from("/proj/lib.ts");
        let records = vec![rec(&file, None, 1, 40)];
        let mut changeset = ChangeSet::from_files([file.clone()]);
        changeset
            .line_ranges
            .insert(file.clone(), vec![LineRange { start: 35, end: 35 }]);

        let items = compute_annotation_items(&records, &[], &changeset);
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains("the module"));
    }

    #[test]
    fn empty_changeset_yields_no_items() {
        let file = PathBuf::from("/proj/lib.ts");
        let records = vec![rec(&file, Some("doThing"), 3, 8)];
        let items = compute_annotation_items(&records, &[], &ChangeSet::default());
        assert!(items.is_empty());
    }
}
