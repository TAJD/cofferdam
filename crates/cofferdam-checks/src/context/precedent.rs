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
//! whole-project bucket, so an all-pairs scan is cheap in practice —
//! but it must run at most once per directory, not once per changed
//! file (CD-203): the cluster depends only on directory membership,
//! so `context_items` caches the per-directory shape list and its
//! all-pairs Jaccard edge set the first time a changed file lands in
//! that directory, and reuses it for every other changed file in the
//! same directory. The changed file itself is excluded only at
//! cluster-selection time (by dropping edges that touch it before
//! running union-find), not by re-running the Jaccard scan.
//!
//! CD-213: same-directory matching structurally can't see a convention
//! that holds across a *kind* of file scattered over many directories
//! (e.g. every `route.ts` under `app/api/**` sharing a response shape)
//! when any single directory only has one or two members of that kind.
//! As a fallback, tried only when a changed file's own directory
//! yields no cluster, files are grouped by exact filename ("kind")
//! across the whole corpus and the same Jaccard clustering runs within
//! that group. This reuses `shape_edges`/`largest_cluster_excluding`
//! unchanged; only the grouping key (filename vs. parent directory)
//! and the cache differ. Directory precedent is preferred when both
//! would fire, since same-directory co-location is the stronger,
//! lower-noise signal.
//!
//! CD-221: a group (directory or kind) emits at most one `ContextItem`
//! per `context_items` call, not one per changed file that lands in
//! it — every changed file belonging to a group is excluded from that
//! group's own clustering (not just whichever one the outer loop
//! happens to be visiting), so a changeset touching several files of
//! the same directory or kind doesn't produce several near-duplicate
//! items differing only in which single file was excluded.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    relevance, Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, CorpusKey,
    FinalizeContext, Issue, Location, RelatedSpan, Severity, SourceFile, Span,
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

/// CD-223: upper bound on a kind group's *shaped* member count before
/// `shape_edges`' O(k^2) all-pairs Jaccard scan is skipped for that
/// group. The same-directory group this scan was originally sized for
/// is naturally bounded (tens of files); CD-213's kind fallback groups
/// by bare filename across the *whole corpus*, and common names
/// (`index.ts`, `types.ts`, `route.ts`) aren't bounded that way — a
/// monorepo can have low-thousands of them. A convention inferred from
/// hundreds of unrelated same-named files isn't a useful signal anyway
/// (precision over recall), so past this size the fallback simply
/// yields nothing for that kind rather than paying the scan.
const MAX_KIND_GROUP_SIZE: usize = 200;

/// Deliberately low and constant — `relevance::FLOOR` (CD-210). Per
/// the product spec, `Context.Precedent` has the lowest source
/// priority of every provider class: it's a sibling-file convention
/// *inference*, not anything traced through the import graph or
/// authored by a human, so it always sorts last (and loses every
/// score tie-break) among items included in the same digest. Every
/// other provider's scoring is required to clamp no lower than
/// `relevance::INFERRED_MIN`, strictly above this constant, so that
/// ordering holds by construction. Note this governs presentation
/// order, not eviction odds — `context_digest::assemble`'s round-robin
/// fairness (CD-211) guarantees Precedent a minimum share of the
/// budget alongside every other provider, so it is not literally
/// "first evicted" under budget pressure — see `relevance::FLOOR`'s
/// doc comment (CD-217).
const SCORE: i32 = relevance::FLOOR;

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
        let mut by_kind: BTreeMap<String, Vec<&FileRecord>> = BTreeMap::new();
        for record in &records {
            if let Some(dir) = record.file.parent() {
                by_dir.entry(dir.to_path_buf()).or_default().push(record);
            }
            by_kind
                .entry(file_label(&record.file))
                .or_default()
                .push(record);
        }
        for siblings in by_dir.values_mut() {
            siblings.sort_by(|a, b| a.file.cmp(&b.file));
        }
        for kind_members in by_kind.values_mut() {
            kind_members.sort_by(|a, b| a.file.cmp(&b.file));
        }

        // Per-directory shape list + all-pairs Jaccard edges, computed at
        // most once per directory regardless of how many changed files
        // land in it (CD-203). Keyed by owned PathBuf since `dir` below
        // borrows from `changeset`, a different lifetime than the
        // `FileRecord` refs borrowed from `records`.
        struct DirShapes<'r> {
            shaped: Vec<(&'r FileRecord, &'r ExportedSymbol)>,
            edges: Vec<(usize, usize)>,
        }
        let mut dir_cache: HashMap<PathBuf, DirShapes<'_>> = HashMap::new();
        let mut kind_cache: HashMap<String, DirShapes<'_>> = HashMap::new();

        // CD-221: every changed file in a group (directory, or kind) is
        // excluded from that group's clustering — not just "whichever
        // changed file the outer loop happens to be on" — and each group
        // produces at most one item, not one per changed file that lands
        // in it. Without this, N changed files sharing a directory or
        // kind each re-run (a cheap, cached) clustering pass and each
        // push a near-duplicate item differing only in which single file
        // was excluded from its own exemplar list.
        let changed_set: std::collections::HashSet<&PathBuf> = changeset.files.iter().collect();
        let mut emitted_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut emitted_kinds: std::collections::HashSet<String> = std::collections::HashSet::new();

        let mut items = Vec::new();
        for changed in &changeset.files {
            let Some(dir) = changed.parent() else {
                continue;
            };

            if let Some(dir_records) = by_dir.get(dir) {
                let dir_shapes = dir_cache.entry(dir.to_path_buf()).or_insert_with(|| {
                    let shaped: Vec<(&FileRecord, &ExportedSymbol)> = dir_records
                        .iter()
                        .filter_map(|r| primary_shape(r).map(|s| (*r, s)))
                        .collect();
                    let edges = shape_edges(&shaped);
                    DirShapes { shaped, edges }
                });

                // Every changed file that landed in this directory's shape
                // list is excluded from consideration, but only at
                // selection time: drop their indices from the
                // effective-count check and skip any precomputed edge
                // touching them before clustering, rather than
                // recomputing the O(siblings²) Jaccard scan.
                let excluded_idx: Vec<usize> = dir_shapes
                    .shaped
                    .iter()
                    .enumerate()
                    .filter(|(_, (r, _))| changed_set.contains(&r.file))
                    .map(|(i, _)| i)
                    .collect();
                let effective_len = dir_shapes.shaped.len() - excluded_idx.len();
                if effective_len >= 2 {
                    if let Some(cluster) = largest_cluster_excluding(
                        dir_shapes.shaped.len(),
                        &dir_shapes.edges,
                        &excluded_idx,
                    ) {
                        let mut exemplars: Vec<&FileRecord> =
                            cluster.iter().map(|&i| dir_shapes.shaped[i].0).collect();
                        exemplars.sort_by(|a, b| a.file.cmp(&b.file));
                        if emitted_dirs.insert(dir.to_path_buf()) {
                            items.push(build_item(dir, &exemplars));
                        }
                        continue;
                    }
                }
            }

            // CD-213 fallback: same-directory matching found nothing for
            // this directory — try grouping by filename ("kind") across
            // the whole corpus instead, so a convention scattered
            // one-per-directory (e.g. `route.ts` under many
            // `app/api/**` subdirectories) can still surface.
            let kind = file_label(changed);
            let Some(kind_records) = by_kind.get(&kind) else {
                continue;
            };
            if kind_records.len() < 2 {
                continue;
            }

            let kind_shapes = kind_cache.entry(kind.clone()).or_insert_with(|| {
                let shaped: Vec<(&FileRecord, &ExportedSymbol)> = kind_records
                    .iter()
                    .filter_map(|r| primary_shape(r).map(|s| (*r, s)))
                    .collect();
                // CD-223: skip the O(k²) scan for an oversized kind group
                // — an empty edge set makes every member its own
                // singleton cluster, so `largest_cluster_excluding`
                // naturally yields `None` below.
                let edges = if shaped.len() > MAX_KIND_GROUP_SIZE {
                    Vec::new()
                } else {
                    shape_edges(&shaped)
                };
                DirShapes { shaped, edges }
            });

            let excluded_idx: Vec<usize> = kind_shapes
                .shaped
                .iter()
                .enumerate()
                .filter(|(_, (r, _))| changed_set.contains(&r.file))
                .map(|(i, _)| i)
                .collect();
            let effective_len = kind_shapes.shaped.len() - excluded_idx.len();
            if effective_len < 2 {
                continue;
            }

            let Some(cluster) = largest_cluster_excluding(
                kind_shapes.shaped.len(),
                &kind_shapes.edges,
                &excluded_idx,
            ) else {
                continue;
            };

            let mut exemplars: Vec<&FileRecord> =
                cluster.iter().map(|&i| kind_shapes.shaped[i].0).collect();
            exemplars.sort_by(|a, b| a.file.cmp(&b.file));

            if emitted_kinds.insert(kind.clone()) {
                items.push(build_kind_item(&kind, &exemplars));
            }
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

/// All index pairs `(i, j)` with `i < j` whose primary shapes meet
/// `SIMILARITY_THRESHOLD`. This is the O(siblings²) all-pairs Jaccard
/// scan — callers compute it once per directory (CD-203) and reuse it
/// for every changed file in that directory via
/// [`largest_cluster_excluding`].
fn shape_edges(shaped: &[(&FileRecord, &ExportedSymbol)]) -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for i in 0..shaped.len() {
        for j in (i + 1)..shaped.len() {
            if jaccard(fields_of(shaped[i].1), fields_of(shaped[j].1)) >= SIMILARITY_THRESHOLD {
                edges.push((i, j));
            }
        }
    }
    edges
}

/// Clusters `n` siblings by the precomputed `edges` (pairs meeting
/// `SIMILARITY_THRESHOLD`) and returns the largest component of size
/// two or more, or `None` when no such component exists. Every index
/// in `excluded` is dropped before clustering: any edge touching one
/// is skipped, so it never joins a component (equivalent to
/// recomputing the Jaccard scan over the sibling set with those files
/// removed, but without re-running the expensive pairwise comparison
/// — Jaccard between two other siblings doesn't depend on whether a
/// third file is present). CD-221: callers pass every *changed* file's
/// index here, not just one — per the module's "sourced entirely from
/// unchanged members" invariant, a changed file must never be cited
/// back as its own precedent's exemplar, whether it's the one caller
/// is currently reporting on or a sibling change in the same
/// changeset. Clusters are sorted by their lowest member index before
/// the size comparison, and `max_by_key` keeps the *last* maximum on
/// ties, so ties on size break on the highest min member index. The
/// result is still deterministic regardless of hash-map iteration
/// order — just not "lowest index wins".
fn largest_cluster_excluding(
    n: usize,
    edges: &[(usize, usize)],
    excluded: &[usize],
) -> Option<Vec<usize>> {
    let excluded: BTreeSet<usize> = excluded.iter().copied().collect();
    let mut dsu = Dsu::new(n);
    for &(i, j) in edges {
        if excluded.contains(&i) || excluded.contains(&j) {
            continue;
        }
        dsu.union(i, j);
    }

    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        if excluded.contains(&i) {
            continue;
        }
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

/// CD-213: same as [`build_item`] but for a cross-directory "kind"
/// match — exemplars live in different directories, so the item lists
/// each exemplar's full path rather than just its filename, and the
/// title/explain text is worded to distinguish it from a same-directory
/// finding.
fn build_kind_item(kind: &str, exemplars: &[&FileRecord]) -> ContextItem {
    let mut body =
        format!("Files named `{kind}` across the project establish a shared export shape:\n\n");
    let mut related = Vec::new();
    for record in exemplars {
        let names: Vec<&str> = record.exports.iter().map(|e| e.name.as_str()).collect();
        body.push_str(&format!(
            "- `{}` exports {}\n",
            record.file.display(),
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
        title: format!("Cross-directory convention for {kind}"),
        body,
        score: SCORE,
        pinned: false,
        related,
        explain: Some(format!(
            "{} file(s) named `{kind}` share a field-shape pattern across directories \
             (Jaccard >= {SIMILARITY_THRESHOLD:.2})",
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
    fn excluding_changed_file_does_not_bridge_a_false_cluster() {
        // CD-203 regression: the changed file (c) is similar enough to
        // BOTH a and b individually to sit in the same connected
        // component as each of them, but a and b are not similar enough
        // to each other. The directory-wide cluster cache must exclude
        // c's edges before clustering — not just drop c from an
        // already-computed {a, b, c} component — or a and b would be
        // wrongly bridged into a false precedent via c.
        let a = PathBuf::from("/p/bridge/a.ts");
        let b = PathBuf::from("/p/bridge/b.ts");
        let c = PathBuf::from("/p/bridge/c.ts");

        let corpus = run_precedent(&[
            (
                &a,
                "export interface A { id: string; name: string; email: string; role: string; \
                 orgId: string; status: string; createdAt: string; updatedAt: string; }",
            ),
            (
                &b,
                "export interface B { id: string; name: string; email: string; role: string; \
                 orgId: string; status: string; deletedAt: string; archivedAt: string; }",
            ),
            (
                &c,
                "export interface C { id: string; name: string; email: string; role: string; \
                 orgId: string; status: string; createdAt: string; deletedAt: string; }",
            ),
        ]);

        let changeset = ChangeSet::from_files([c.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert!(
            items.is_empty(),
            "a and b must not be bridged into a false precedent via c; got {items:?}"
        );
    }

    #[test]
    fn kind_group_larger_than_the_cap_yields_no_item_instead_of_running_the_full_scan() {
        // CD-223: a kind group larger than MAX_KIND_GROUP_SIZE skips the
        // O(k^2) Jaccard scan entirely, so even a genuine shared shape
        // across every member produces nothing — precision over recall,
        // and avoids an unbounded pairwise scan on a monorepo full of
        // e.g. `index.ts`.
        let shape_src =
            "export interface RouteResponse { data: string; status: string; error: string; }";
        let mut fixtures: Vec<(PathBuf, &str)> = (0..=MAX_KIND_GROUP_SIZE)
            .map(|i| (PathBuf::from(format!("/p/mod{i}/route.ts")), shape_src))
            .collect();
        let changed = PathBuf::from("/p/changed_mod/route.ts");
        fixtures.push((
            changed.clone(),
            "export async function GET(): Promise<void> {}",
        ));

        let fixture_refs: Vec<(&Path, &str)> =
            fixtures.iter().map(|(p, s)| (p.as_path(), *s)).collect();
        let corpus = run_precedent(&fixture_refs);

        let changeset = ChangeSet::from_files([changed]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert!(
            items.is_empty(),
            "kind group over the cap must yield nothing; got {items:?}"
        );
    }

    #[test]
    fn cross_directory_kind_match_surfaces_when_own_directory_has_no_siblings() {
        // CD-213: three `route.ts` files, one per directory (so
        // same-directory matching sees no siblings anywhere), sharing a
        // response shape. The changed file is a 4th `route.ts` in yet
        // another lone directory with no shape of its own.
        let a = PathBuf::from("/p/app/api/users/route.ts");
        let b = PathBuf::from("/p/app/api/orgs/route.ts");
        let c = PathBuf::from("/p/app/api/teams/route.ts");
        let changed = PathBuf::from("/p/app/api/invites/route.ts");

        let corpus = run_precedent(&[
            (
                &a,
                "export interface RouteResponse { data: string; status: string; error: string; }",
            ),
            (
                &b,
                "export interface RouteResponse { data: string; status: string; error: string; }",
            ),
            (
                &c,
                "export interface RouteResponse { data: string; status: string; error: string; }",
            ),
            (&changed, "export async function GET(): Promise<void> {}"),
        ]);

        let changeset = ChangeSet::from_files([changed.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert_eq!(
            items.len(),
            1,
            "expected one cross-directory item; got {items:?}"
        );
        let item = &items[0];
        assert_eq!(item.check_id, "Context.Precedent");
        assert!(item.title.contains("route.ts"));
        assert!(
            item.body.contains("app/api/users/route.ts")
                || item.body.contains("app\\api\\users\\route.ts")
        );
        // The changed file itself must never appear as an exemplar.
        assert!(!item.body.contains("invites"));
    }

    #[test]
    fn same_directory_match_is_preferred_over_cross_directory_kind_match() {
        // A changed file whose own directory already has an established
        // convention must use that, not fall through to the (here,
        // absent) kind-based grouping.
        let a = PathBuf::from("/p/handlers/create_user.ts");
        let b = PathBuf::from("/p/handlers/update_user.ts");
        let changed = PathBuf::from("/p/handlers/delete_user.ts");

        let corpus = run_precedent(&[
            (&a, create_user_src()),
            (&b, update_user_src()),
            (
                &changed,
                "export async function deleteUser(id: string): Promise<void> {}",
            ),
        ]);

        let changeset = ChangeSet::from_files([changed.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert_eq!(items.len(), 1);
        assert!(items[0].title.starts_with("Sibling convention"));
    }

    #[test]
    fn multiple_changed_files_of_the_same_kind_produce_one_item_not_one_per_file() {
        // CD-221 regression: 5 `route.ts` files sharing a shape, spread
        // one-per-directory (own-directory clustering can't see them),
        // with 2 of them landing in the same changeset. Must produce
        // exactly one cross-directory item, and neither changed file may
        // appear as its own exemplar.
        let dirs = ["users", "orgs", "teams", "invites", "bots"];
        let files: Vec<PathBuf> = dirs
            .iter()
            .map(|d| PathBuf::from(format!("/p/app/api/{d}/route.ts")))
            .collect();
        let fixtures: Vec<(&Path, &str)> = files
            .iter()
            .map(|f| {
                (
                    f.as_path(),
                    "export interface RouteResponse { data: string; status: string; error: string; }",
                )
            })
            .collect();
        let corpus = run_precedent(&fixtures);

        let changeset = ChangeSet::from_files([files[3].clone(), files[4].clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert_eq!(
            items.len(),
            1,
            "expected exactly one dedup'd item; got {items:?}"
        );
        assert!(!items[0].body.contains("invites"));
        assert!(!items[0].body.contains("bots"));
    }

    #[test]
    fn multiple_changed_files_in_same_directory_produce_one_item_not_one_per_file() {
        // Milder pre-existing version of CD-221, for the directory path:
        // two changed files landing in the same directory that also has
        // an established convention must not each independently cite the
        // other back as its own exemplar. Needs two *unchanged* siblings
        // remaining after excluding both changed files, or no cluster is
        // even possible (that's the pre-existing "< 2 shaped siblings"
        // rule, unrelated to this fix).
        let a = PathBuf::from("/p/handlers/create_user.ts");
        let b = PathBuf::from("/p/handlers/update_user.ts");
        let c = PathBuf::from("/p/handlers/list_users.ts");
        let d = PathBuf::from("/p/handlers/get_user.ts");

        let get_user_src = "export interface GetUserRequest { id: string; name: string; email: string; role: string; orgId: string; }\n\
             export async function getUser(req: GetUserRequest): Promise<string> { return req.id; }";

        let corpus = run_precedent(&[
            (&a, create_user_src()),
            (&b, update_user_src()),
            (&c, list_users_src()),
            (&d, get_user_src),
        ]);

        let changeset = ChangeSet::from_files([a.clone(), b.clone()]);
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let items = Precedent.context_items(&changeset, &mut finalize_ctx);

        assert_eq!(
            items.len(),
            1,
            "expected exactly one dedup'd item; got {items:?}"
        );
        assert!(!items[0].body.contains("create_user.ts"));
        assert!(!items[0].body.contains("update_user.ts"));
        assert!(items[0].body.contains("list_users.ts"));
        assert!(items[0].body.contains("get_user.ts"));
    }

    #[test]
    fn jaccard_matches_known_values() {
        let a: BTreeSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: BTreeSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - (2.0 / 3.0)).abs() < 1e-9);
        assert_eq!(jaccard(&BTreeSet::new(), &BTreeSet::new()), 0.0);
    }
}
