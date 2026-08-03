//! `Context.Precedent` (CD-161, CP5) — advisory `cofferdam context`
//! provider that points at sibling-file conventions.
//!
//! For every changed file, looks at the other files in the same
//! directory ("module") and asks: do at least two of them export an
//! interface / object-literal type alias with a near-identical field
//! shape? If so, that's an established convention — the provider emits
//! one `ContextItem` per changed file naming the exemplar sibling files
//! and their exported symbols (not code). If no such cluster exists
//! (fewer than two shaped siblings, or none cluster above the
//! threshold), nothing is emitted — precision over recall, per the
//! product spec (wiki `cofferdam-context-product-criteria`).
//!
//! Deliberately does not require the changed file itself to already
//! match the pattern: the acceptance criterion is a *new* file landing
//! in a module with an established convention, which may have no shape
//! of its own yet. The signal is "this module has a convention",
//! sourced entirely from its other (unchanged) members.
//!
//! "Module" is approximated as "same parent directory" rather than a
//! `cofferdam.invariants.toml` layer — this keeps the provider useful
//! with zero config (per the product spec's zero-config value bar) and
//! avoids re-deriving layer membership that `Design.LayerViolation`
//! already owns for a different purpose.
//!
//! Shape similarity reuses the field-name-set heuristic
//! `Design.DuplicateTypeShape` (crates/cofferdam-checks/src/design/duplicate_type_shape.rs)
//! is built on, simplified to a plain Jaccard index over field names
//! (no field-type-text comparison, no `extends`-bucket handling) — this
//! provider is a coarse "these look alike" nudge, not a duplication
//! flag, so the cheaper metric is the right tradeoff. The optimized
//! prefix-filtering join from CD-193 isn't reused either: candidate
//! sets here are a handful of same-directory siblings, not a
//! whole-project bucket, so an all-pairs scan is already O(1) in
//! practice.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, CorpusKey, FinalizeContext,
    Issue, Location, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    Declaration, ExportNamedDeclaration, Expression, PropertyKey, TSSignature, TSType,
};
use oxc_ast_visit::Visit;

/// Below this field count, a shape is presumed too generic to be a
/// meaningful convention signal (mirrors `Design.DuplicateTypeShape`'s
/// `MIN_FIELDS`, though this provider's threshold is its own tunable —
/// the two checks solve different problems and shouldn't be coupled).
const MIN_FIELDS: usize = 2;

/// Jaccard similarity (over field names) two siblings' primary shapes
/// must reach to be considered "the same convention". Looser than
/// `Design.DuplicateTypeShape`'s 0.8 duplication threshold — this
/// provider is flagging a family resemblance, not a likely-should-be-
/// merged duplicate.
const SIMILARITY_THRESHOLD: f64 = 0.75;

/// Deliberately low and constant. Per the product spec,
/// `Context.Precedent` has the lowest source priority of the four
/// provider classes (Findings, BlastRadius, Precedent, Knowledge) and
/// is the first evicted under a tight `--budget`. When the other three
/// providers land they should use materially higher scores; this value
/// only needs to stay below theirs, not carry any other meaning.
const SCORE: i32 = 5;

/// What a single exported top-level symbol looks like, for the
/// purposes of "do these two files follow the same convention".
#[derive(Clone)]
enum ExportShape {
    /// An exported `interface` or `type X = { ... }` object-literal
    /// alias — the field-name set is the comparable signal.
    Fields(BTreeSet<String>),
    /// An exported function declaration or a `const` bound to an arrow
    /// / function expression. Not used for similarity — only listed by
    /// name in a finding's body — so no further detail is captured.
    Function,
    /// An exported class declaration.
    Class,
    /// Any other exported top-level binding (a plain `const`, an
    /// enum, a non-object-literal type alias, ...). Not used for
    /// similarity, only listed by name in a finding's body.
    Other,
}

#[derive(Clone)]
struct ExportedSymbol {
    name: String,
    span: Span,
    shape: ExportShape,
}

#[derive(Clone)]
struct FileRecord {
    file: PathBuf,
    exports: Vec<ExportedSymbol>,
}

static PRECEDENT_EXPORTS: CorpusKey<Vec<FileRecord>> = CorpusKey::new("Context.Precedent.exports");

const META: CheckMeta = CheckMeta {
    id: "Context.Precedent",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "Advisory: sibling files in the same directory establish a shared export \
        shape — surfaces the nearest-neighbor exemplars so a new file added to that module \
        follows the existing convention.",
    // Context providers aren't part of the `cofferdam gen-docs` catalog
    // (they're excluded from `all_builtins()`, which is gen-docs's only
    // input), so — unlike an ordinary check's `META.body` — this is a
    // plain string literal rather than an `include_str!` companion
    // file: there is no catalog page for it to back, and creating one
    // under `docs/` would trip `no_orphan_companion_files` in
    // `crates/cofferdam-checks/src/lib.rs` (every file there must
    // correspond to a registered `all_builtins()` id).
    body: "Groups sibling files (same parent directory) by the field-name shape of their \
        largest exported interface / object-literal type alias. When at least two siblings \
        of a changed file's directory share a shape (Jaccard similarity over field names at \
        or above threshold), emits one pointer per changed file naming the exemplar files \
        and their exported symbols — never a code dump. Emits nothing when the directory has \
        fewer than two shaped siblings, or no cluster reaches the similarity threshold: \
        precision over recall, an empty digest section is valid and expected for most edits.",
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes per-file FileRecords into the corpus during run(); skipping
    // run() on a cache hit would drop that file's contribution and
    // silently under-report at context_items() time, mirroring every
    // other corpus-writing check in this crate.
    pure_run: false,
};

/// `Context.Precedent` — CD-156/CD-161. See module docs.
pub struct Precedent;

impl Check for Precedent {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&PRECEDENT_EXPORTS, |slot, path| {
            slot.retain(|r| r.file != path);
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut collector = ExportShapeCollector {
            file,
            exports: Vec::new(),
        };
        collector.visit_program(parsed.program);
        if !collector.exports.is_empty() {
            ctx.corpus.with_slot(&PRECEDENT_EXPORTS, |slot| {
                slot.push(FileRecord {
                    file: file.path.clone(),
                    exports: collector.exports,
                });
            });
        }
        Vec::new()
    }

    fn context_items(
        &self,
        changeset: &ChangeSet,
        ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        let records: Vec<FileRecord> = ctx
            .corpus
            .with_slot(&PRECEDENT_EXPORTS, |slot| slot.clone());

        let mut by_dir: BTreeMap<PathBuf, Vec<&FileRecord>> = BTreeMap::new();
        for record in &records {
            if let Some(dir) = record.file.parent() {
                by_dir.entry(dir.to_path_buf()).or_default().push(record);
            }
        }
        for siblings in by_dir.values_mut() {
            siblings.sort_by(|a, b| a.file.cmp(&b.file));
        }

        let mut items = Vec::new();
        for changed in &changeset.files {
            let Some(dir) = changed.parent() else {
                continue;
            };
            let Some(dir_records) = by_dir.get(dir) else {
                continue;
            };

            let shaped: Vec<(&FileRecord, &ExportedSymbol)> = dir_records
                .iter()
                .filter(|r| r.file != *changed)
                .filter_map(|r| primary_shape(r).map(|s| (*r, s)))
                .collect();
            if shaped.len() < 2 {
                continue;
            }

            let Some(cluster) = largest_shape_cluster(&shaped) else {
                continue;
            };

            let mut exemplars: Vec<&FileRecord> = cluster.iter().map(|&i| shaped[i].0).collect();
            exemplars.sort_by(|a, b| a.file.cmp(&b.file));

            items.push(build_item(dir, &exemplars));
        }
        items
    }
}

/// The exported interface/type-literal with the most fields in this
/// file's record (`>= MIN_FIELDS`), or `None` when the file exports no
/// such shape. Ties break on first-seen (declaration order).
fn primary_shape(record: &FileRecord) -> Option<&ExportedSymbol> {
    let mut best: Option<&ExportedSymbol> = None;
    for sym in &record.exports {
        if let ExportShape::Fields(fields) = &sym.shape {
            if fields.len() < MIN_FIELDS {
                continue;
            }
            let better = match best {
                None => true,
                Some(current) => fields.len() > fields_of(current).len(),
            };
            if better {
                best = Some(sym);
            }
        }
    }
    best
}

fn fields_of(sym: &ExportedSymbol) -> &BTreeSet<String> {
    match &sym.shape {
        ExportShape::Fields(f) => f,
        _ => unreachable!("fields_of called on a non-Fields ExportShape"),
    }
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Minimal union-find, local to this module (not shared with
/// `Design.DuplicateTypeShape`'s private `Dsu` — different module,
/// and small enough that duplicating it beats threading a `pub(crate)`
/// surface across an unrelated check for a dozen lines).
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Clusters `shaped` siblings (index-aligned pairs of file + its
/// primary shape) by pairwise Jaccard similarity and returns the
/// largest component of size >= 2, or `None` when no such component
/// exists. Clusters are sorted by their lowest member index before
/// the size comparison, and `max_by_key` keeps the *last* maximum on
/// ties, so ties on size break on the highest min member index. The
/// result is still deterministic regardless of hash-map iteration
/// order — just not "lowest index wins".
fn largest_shape_cluster(shaped: &[(&FileRecord, &ExportedSymbol)]) -> Option<Vec<usize>> {
    let mut dsu = Dsu::new(shaped.len());
    for i in 0..shaped.len() {
        for j in (i + 1)..shaped.len() {
            if jaccard(fields_of(shaped[i].1), fields_of(shaped[j].1)) >= SIMILARITY_THRESHOLD {
                dsu.union(i, j);
            }
        }
    }

    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..shaped.len() {
        by_root.entry(dsu.find(i)).or_default().push(i);
    }

    let mut clusters: Vec<Vec<usize>> = by_root.into_values().filter(|c| c.len() >= 2).collect();
    clusters.sort_by_key(|c| *c.iter().min().expect("non-empty cluster"));
    clusters.into_iter().max_by_key(|c| c.len())
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn build_item(dir: &Path, exemplars: &[&FileRecord]) -> ContextItem {
    let dir_display = dir.display();
    let mut body = format!("Sibling files in `{dir_display}` establish a shared export shape:\n\n");
    let mut related = Vec::new();
    for record in exemplars {
        let names: Vec<&str> = record.exports.iter().map(|e| e.name.as_str()).collect();
        body.push_str(&format!(
            "- `{}` exports {}\n",
            file_label(&record.file),
            names.join(", ")
        ));
        if let Some(shape_sym) = primary_shape(record) {
            related.push(RelatedSpan {
                file: record.file.clone(),
                location: Location::from_span(&record.file, shape_sym.span),
            });
        }
    }
    ContextItem {
        check_id: META.id.to_string(),
        title: format!("Sibling convention in {dir_display}"),
        body,
        score: SCORE,
        pinned: false,
        related,
        explain: Some(format!(
            "{} sibling file(s) in `{dir_display}` share a field-shape pattern (Jaccard >= \
             {SIMILARITY_THRESHOLD:.2})",
            exemplars.len()
        )),
    }
}

fn collect_field_names(members: &[TSSignature<'_>]) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    for member in members {
        if let TSSignature::TSPropertySignature(prop) = member {
            if let PropertyKey::StaticIdentifier(id) = &prop.key {
                fields.insert(id.name.to_string());
            }
        }
    }
    fields
}

struct ExportShapeCollector<'a> {
    file: &'a SourceFile,
    exports: Vec<ExportedSymbol>,
}

impl<'a> ExportShapeCollector<'a> {
    fn record(&mut self, name: &str, start: u32, end: u32, shape: ExportShape) {
        self.exports.push(ExportedSymbol {
            name: name.to_string(),
            span: span_from_bytes(&self.file.text, start, end),
            shape,
        });
    }
}

impl<'a> Visit<'a> for ExportShapeCollector<'a> {
    fn visit_export_named_declaration(&mut self, node: &ExportNamedDeclaration<'a>) {
        if let Some(decl) = &node.declaration {
            match decl {
                Declaration::FunctionDeclaration(f) => {
                    if let Some(id) = &f.id {
                        self.record(
                            id.name.as_str(),
                            id.span.start,
                            id.span.end,
                            ExportShape::Function,
                        );
                    }
                }
                Declaration::ClassDeclaration(c) => {
                    if let Some(id) = &c.id {
                        self.record(
                            id.name.as_str(),
                            id.span.start,
                            id.span.end,
                            ExportShape::Class,
                        );
                    }
                }
                Declaration::TSInterfaceDeclaration(d) => {
                    let fields = collect_field_names(&d.body.body);
                    self.record(
                        d.id.name.as_str(),
                        d.id.span.start,
                        d.id.span.end,
                        ExportShape::Fields(fields),
                    );
                }
                Declaration::TSTypeAliasDeclaration(d) => {
                    let shape = match &d.type_annotation {
                        TSType::TSTypeLiteral(lit) => {
                            ExportShape::Fields(collect_field_names(&lit.members))
                        }
                        _ => ExportShape::Other,
                    };
                    self.record(d.id.name.as_str(), d.id.span.start, d.id.span.end, shape);
                }
                Declaration::VariableDeclaration(v) => {
                    for declarator in &v.declarations {
                        if let Some(ident) = declarator.id.get_binding_identifier() {
                            let shape = match declarator.init.as_ref() {
                                Some(Expression::ArrowFunctionExpression(_)) => {
                                    ExportShape::Function
                                }
                                Some(Expression::FunctionExpression(_)) => ExportShape::Function,
                                _ => ExportShape::Other,
                            };
                            self.record(
                                ident.name.as_str(),
                                ident.span.start,
                                ident.span.end,
                                shape,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        oxc_ast_visit::walk::walk_export_named_declaration(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{Allocator, CorpusIndex};

    fn run_precedent(fixtures: &[(&Path, &str)]) -> CorpusIndex {
        let corpus = CorpusIndex::new();
        for (path, src) in fixtures {
            let file = SourceFile::new(path.to_path_buf(), (*src).to_string());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            Precedent.run(&file, &mut ctx);
        }
        corpus
    }

    fn create_user_src() -> &'static str {
        "export interface CreateUserRequest { name: string; email: string; role: string; orgId: string; }\n\
         export async function createUser(req: CreateUserRequest): Promise<string> { return req.name; }"
    }

    fn update_user_src() -> &'static str {
        "export interface UpdateUserRequest { id: string; name: string; email: string; role: string; orgId: string; }\n\
         export async function updateUser(req: UpdateUserRequest): Promise<string> { return req.id; }"
    }

    fn list_users_src() -> &'static str {
        "export interface ListUsersRequest { orgId: string; role: string; name: string; email: string; }\n\
         export async function listUsers(req: ListUsersRequest): Promise<string[]> { return []; }"
    }

    #[test]
    fn established_pattern_surfaces_when_a_new_file_is_added() {
        let a = PathBuf::from("/p/handlers/create_user.ts");
        let b = PathBuf::from("/p/handlers/update_user.ts");
        let c = PathBuf::from("/p/handlers/list_users.ts");
        let new_file = PathBuf::from("/p/handlers/delete_user.ts");

        let corpus = run_precedent(&[
            (&a, create_user_src()),
            (&b, update_user_src()),
            (&c, list_users_src()),
            (
                &new_file,
                "export async function deleteUser(id: string): Promise<void> {}",
            ),
        ]);

        let changeset = ChangeSet::from_files([new_file.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert_eq!(items.len(), 1, "expected one precedent item; got {items:?}");
        let item = &items[0];
        assert_eq!(item.check_id, "Context.Precedent");
        assert!(!item.pinned);
        assert!(item.body.contains("create_user.ts"));
        assert!(item.body.contains("CreateUserRequest"));
        assert!(item.body.contains("update_user.ts") || item.body.contains("list_users.ts"));
        assert!(item.explain.is_some());
        // The changed file itself must never appear as an exemplar.
        assert!(!item.body.contains("delete_user.ts"));
    }

    #[test]
    fn unrelated_edit_produces_no_precedent_items() {
        // A lone file with no siblings at all — no established pattern
        // is even possible.
        let solo = PathBuf::from("/p/utils/format.ts");
        let corpus = run_precedent(&[(
            &solo,
            "export function formatName(n: string): string { return n; }",
        )]);

        let changeset = ChangeSet::from_files([solo.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert!(items.is_empty(), "expected no items; got {items:?}");
    }

    #[test]
    fn dissimilar_siblings_produce_no_precedent_items() {
        let a = PathBuf::from("/p/mixed/one.ts");
        let b = PathBuf::from("/p/mixed/two.ts");
        let changed = PathBuf::from("/p/mixed/three.ts");

        let corpus = run_precedent(&[
            (
                &a,
                "export interface Draft { id: string; title: string; body: string; }",
            ),
            (
                &b,
                "export interface Settings { theme: string; locale: string; timezone: string; }",
            ),
            (&changed, "export function noop(): void {}"),
        ]);

        let changeset = ChangeSet::from_files([changed.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert!(
            items.is_empty(),
            "dissimilar siblings must not cluster into a false precedent; got {items:?}"
        );
    }

    #[test]
    fn a_single_shaped_sibling_is_not_an_established_pattern() {
        // Only one other file in the directory exports a shape — one
        // occurrence isn't a "pattern" yet.
        let a = PathBuf::from("/p/lonely/one.ts");
        let changed = PathBuf::from("/p/lonely/two.ts");

        let corpus = run_precedent(&[
            (
                &a,
                "export interface Config { host: string; port: string; }",
            ),
            (&changed, "export function noop(): void {}"),
        ]);

        let changeset = ChangeSet::from_files([changed.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert!(items.is_empty(), "expected no items; got {items:?}");
    }

    #[test]
    fn jaccard_matches_known_values() {
        let a: BTreeSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: BTreeSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - (2.0 / 3.0)).abs() < 1e-9);
        assert_eq!(jaccard(&BTreeSet::new(), &BTreeSet::new()), 0.0);
    }
}
