use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    Priority, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    Declaration, PropertyKey, TSInterfaceDeclaration, TSSignature, TSType, TSTypeAliasDeclaration,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

/// Below this field count, a shape match is presumed coincidental (e.g.
/// two unrelated `{ id: string }` types) rather than real duplication.
const MIN_FIELDS: usize = 3;

/// Fraction of the union of both types' field names that must match on
/// both name and type text for the pair to be flagged. Tuned loose
/// enough to catch near-duplicates (an extra/renamed field) without
/// flagging coincidentally-overlapping-but-different shapes.
const SIMILARITY_THRESHOLD: f64 = 0.8;

/// One observed interface/type-literal declaration, collected during the
/// per-file pass. `fields` maps property name to its type annotation's
/// raw source text (trimmed) — deliberately textual rather than
/// semantic, since resolving type equivalence would need the ts-morph
/// type host this check doesn't require. `extends` holds the sorted,
/// raw-text names of an interface's heritage clauses (empty for a type
/// alias, or an interface with no `extends`).
#[derive(Clone)]
struct TypeShape {
    name: String,
    file: PathBuf,
    span: Span,
    fields: BTreeMap<String, String>,
    extends: Vec<String>,
}

/// A shape with no own fields is only a comparison candidate when it has
/// a non-empty `extends` — a shared base is itself a duplication signal
/// (CD-135) even with zero additional fields, whereas an empty type
/// alias/interface with no base is not comparable to anything.
fn is_eligible(shape: &TypeShape) -> bool {
    !shape.extends.is_empty() || shape.fields.len() >= MIN_FIELDS
}

static TYPE_SHAPES: CorpusKey<Vec<TypeShape>> = CorpusKey::new("Design.DuplicateTypeShape.shapes");

const META: CheckMeta = CheckMeta {
    id: "Design.DuplicateTypeShape",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "Two independently declared interfaces/type literals share \
        (near-)identical field shapes under different names — likely should \
        be a single shared type.",
    body: include_str!("../../docs/Design.DuplicateTypeShape.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes per-file TypeShapes into the corpus during run(); skipping
    // run() on cache hit would drop that file's contribution and
    // silently under-report in finalize(), mirroring
    // Design.DuplicateExportName.
    pure_run: false,
};

/// `Design.DuplicateTypeShape` — cross-file check (CD-128) that flags
/// groups of interfaces/type-literal aliases with sufficiently similar
/// field shapes.
///
/// Scope: only `interface Foo { ... }` and `type Foo = { ... }`
/// declarations are considered — mapped types, unions, and other TSType
/// forms are not. An interface with a non-empty `extends` clause is only
/// ever compared against another interface with the exact same
/// (sorted) set of heritage names (CD-135) — its effective shape
/// includes inherited fields this check doesn't resolve, so comparing
/// declared bodies across different bases would risk false
/// positives/negatives; when both sides share a base, that shared base
/// is itself a duplication signal even if neither side adds its own
/// fields. Only plain identifier property keys are counted;
/// computed/private keys are ignored, so a shape with only such keys
/// will never reach `MIN_FIELDS` (unless it has a non-empty `extends`).
/// Comparison is pairwise, but only within an `extends` bucket and only
/// across a field-count band that provably brackets every pair able to
/// reach the threshold (CD-180), so the all-pairs blow-up is avoided;
/// mutually-similar shapes are transitively clustered via union-find:
/// three mutually-similar types produce one finding naming all three,
/// not three separate pairwise findings.
pub struct DuplicateTypeShape;

impl Check for DuplicateTypeShape {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&TYPE_SHAPES, |slot, path| slot.retain(|s| s.file != path));
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut collector = ShapeCollector {
            file,
            shapes: Vec::new(),
        };
        collector.visit_program(parsed.program);
        ctx.corpus.with_slot(&TYPE_SHAPES, |slot| {
            slot.append(&mut collector.shapes);
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let mut shapes: Vec<TypeShape> = ctx.corpus.with_slot(&TYPE_SHAPES, |slot| slot.clone());
        shapes.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
        });
        compute_duplicates(&shapes)
    }
}

/// `a`/`b` are only comparable when they share the exact same (sorted)
/// `extends` set — a differing base means the effective (inherited)
/// shape differs even if the declared bodies happen to match.
///
/// `fields` is a `BTreeMap`, so both key sequences are already sorted;
/// the union size and the matching count are obtained from a single
/// allocation-free two-pointer merge rather than by materialising the
/// union as a `HashSet` per pair (CD-180 — this runs once per candidate
/// pair, so the per-pair allocation dominated finalize at 5k files).
fn similarity(a: &TypeShape, b: &TypeShape) -> f64 {
    if a.extends != b.extends {
        return 0.0;
    }
    let mut union_len = 0usize;
    let mut matching = 0usize;
    let mut ia = a.fields.iter().peekable();
    let mut ib = b.fields.iter().peekable();
    loop {
        let ord = match (ia.peek(), ib.peek()) {
            (None, None) => break,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some((ka, _)), Some((kb, _))) => ka.cmp(kb),
        };
        union_len += 1;
        match ord {
            Ordering::Less => {
                ia.next();
            }
            Ordering::Greater => {
                ib.next();
            }
            Ordering::Equal => {
                let (_, va) = ia.next().expect("peeked");
                let (_, vb) = ib.next().expect("peeked");
                if va == vb {
                    matching += 1;
                }
            }
        }
    }
    if union_len == 0 {
        // Both sides declare no fields of their own. If they also share
        // a non-empty extends clause, that shared base alone is a full
        // duplication signal (CD-135); otherwise there's nothing to
        // compare.
        return if a.extends.is_empty() { 0.0 } else { 1.0 };
    }
    matching as f64 / union_len as f64
}

/// Minimal union-find over shape indices, used to transitively cluster
/// mutually-similar shapes (CD-135) instead of reporting one finding per
/// pairwise match.
struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Self {
        Dsu {
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

/// Two shapes can only reach `SIMILARITY_THRESHOLD` when their declared
/// field counts are within a bounded ratio of each other, which makes
/// the eligible pairs a sliding window over a count-sorted bucket
/// (CD-180). Derivation: `matching <= min(|a|, |b|)` and
/// `union_len >= max(|a|, |b|)`, so
/// `similarity = matching/union_len <= min/max`. Therefore
/// `similarity >= T` implies `min/max >= T`, and any pair with
/// `min/max < T` can be skipped without computing its true similarity.
/// The comparison is deliberately slack by `EPSILON` so that a
/// floating-point rounding artefact can only ever *widen* the window
/// (keeping a pair whose exact similarity is then computed as usual),
/// never drop a real match.
///
/// The zero-field case needs no special handling: `min/max` is `0/0`
/// only when both sides declare no fields, and `0.0 * T > 0.0 + EPSILON`
/// is false, so such pairs fall through to `similarity`, which applies
/// the shared-`extends` rule (CD-135).
fn count_band_excludes(smaller: usize, larger: usize) -> bool {
    const EPSILON: f64 = 1e-9;
    larger as f64 * SIMILARITY_THRESHOLD > smaller as f64 + EPSILON
}

fn compute_duplicates(shapes: &[TypeShape]) -> Vec<Issue> {
    let mut dsu = Dsu::new(shapes.len());

    // `similarity` returns 0.0 outright for a differing `extends`, so
    // shapes in different `extends` buckets are provably never similar
    // and never need comparing (CD-180).
    let mut buckets: HashMap<&[String], Vec<usize>> = HashMap::new();
    for (i, shape) in shapes.iter().enumerate() {
        if is_eligible(shape) {
            buckets.entry(shape.extends.as_slice()).or_default().push(i);
        }
    }

    for mut bucket in buckets.into_values() {
        bucket.sort_by_key(|&i| shapes[i].fields.len());
        for (pos, &i) in bucket.iter().enumerate() {
            let count_i = shapes[i].fields.len();
            for &j in &bucket[(pos + 1)..] {
                // `bucket` is sorted ascending by field count, so every
                // remaining `j` has `count_j >= count_i` and the counts
                // only grow — once one leaves the band, so does the rest
                // of the tail.
                if count_band_excludes(count_i, shapes[j].fields.len()) {
                    break;
                }
                if similarity(&shapes[i], &shapes[j]) >= SIMILARITY_THRESHOLD {
                    dsu.union(i, j);
                }
            }
        }
    }

    build_issues(shapes, &mut dsu)
}

/// Group eligible indices into connected components and emit one `Issue`
/// per multi-member group. `shapes` is already sorted by (file, span),
/// so each group's members come out in that order and the first member
/// is a stable, deterministic anchor. Groups are ordered by their lowest
/// member index rather than by DSU root, so the output does not depend
/// on the order in which unions happened to be applied.
fn build_issues(shapes: &[TypeShape], dsu: &mut Dsu) -> Vec<Issue> {
    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, shape) in shapes.iter().enumerate() {
        if is_eligible(shape) {
            by_root.entry(dsu.find(i)).or_default().push(i);
        }
    }
    let mut groups: Vec<Vec<usize>> = by_root.into_values().collect();
    groups.sort_by_key(|members| members[0]);

    let mut issues = Vec::new();
    for members in groups {
        if members.len() < 2 {
            continue;
        }
        let anchor = &shapes[members[0]];
        let others: Vec<&TypeShape> = members[1..].iter().map(|&idx| &shapes[idx]).collect();
        let other_names = others
            .iter()
            .map(|s| format!("`{}` in {}", s.name, s.file.display()))
            .collect::<Vec<_>>()
            .join(", ");
        let plural = if others.len() == 1 { "" } else { "s" };
        // A group can only reach here with zero declared fields via the
        // shared-extends-only signal (similarity() only returns 1.0 for
        // an empty-fields pair when both share a non-empty extends) — so
        // an empty-fields anchor means every member here shares a base,
        // not a field shape, and the message should say so.
        let message = if anchor.fields.is_empty() {
            format!(
                "`{}` extends the same base (`{}`) as {} other type{}: {} — consider a shared type",
                anchor.name,
                anchor.extends.join(", "),
                others.len(),
                plural,
                other_names
            )
        } else {
            format!(
                "`{}` ({} fields) shares a near-identical field shape with {} other type{}: {} — consider a shared type",
                anchor.name,
                anchor.fields.len(),
                others.len(),
                plural,
                other_names
            )
        };
        issues.push(Issue {
            check_id: META.id.to_string(),
            message,
            file: anchor.file.clone(),
            location: Location::from_span(&anchor.file, anchor.span),
            priority: Priority(META.base_priority),
            severity: Severity::Medium,
            related: others
                .iter()
                .map(|s| RelatedSpan {
                    file: s.file.clone(),
                    location: Location::from_span(&s.file, s.span),
                })
                .collect(),
        });
    }
    issues
}

/// Raw source text of a property signature's type annotation, or an
/// empty string for an implicit-any field with no annotation.
fn field_type_text<'a>(file: &SourceFile, ty: Option<&TSType<'a>>) -> String {
    match ty {
        Some(t) => file
            .text
            .get(t.span().start as usize..t.span().end as usize)
            .unwrap_or("")
            .trim()
            .to_string(),
        None => String::new(),
    }
}

fn collect_fields(file: &SourceFile, members: &[TSSignature<'_>]) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for member in members {
        if let TSSignature::TSPropertySignature(prop) = member {
            if let PropertyKey::StaticIdentifier(id) = &prop.key {
                let type_text = field_type_text(
                    file,
                    prop.type_annotation.as_ref().map(|a| &a.type_annotation),
                );
                fields.insert(id.name.to_string(), type_text);
            }
        }
    }
    fields
}

struct ShapeCollector<'a> {
    file: &'a SourceFile,
    shapes: Vec<TypeShape>,
}

impl<'a> Visit<'a> for ShapeCollector<'a> {
    fn visit_declaration(&mut self, decl: &Declaration<'a>) {
        match decl {
            Declaration::TSInterfaceDeclaration(d) => self.record_interface(d),
            Declaration::TSTypeAliasDeclaration(d) => self.record_type_alias(d),
            _ => {}
        }
        oxc_ast_visit::walk::walk_declaration(self, decl);
    }
}

/// Raw source text of each heritage clause's expression (e.g. `Base` in
/// `extends Base`), sorted for order-independent comparison. Only the
/// callee expression is sliced, not `heritage.type_arguments` — so
/// `extends Base<string>` and `extends Base<number>` (or plain `Base`)
/// compare as the same base. Harmless for the field-ratio path (the
/// generic's own fields still have to match), but a zero-field extender
/// of `Base<string>` and one of `Base<number>` would be treated as
/// sharing a base when they may not.
fn extends_names(file: &SourceFile, decl: &TSInterfaceDeclaration<'_>) -> Vec<String> {
    let mut names: Vec<String> = decl
        .extends
        .iter()
        .map(|heritage| {
            let span = heritage.expression.span();
            file.text
                .get(span.start as usize..span.end as usize)
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

impl<'a> ShapeCollector<'a> {
    fn record_interface(&mut self, decl: &TSInterfaceDeclaration<'a>) {
        // Inherited fields from `extends` aren't visible in the body, so
        // an interface's own declared field count alone doesn't
        // determine eligibility here — a non-empty extends is itself a
        // duplication signal (CD-135), gated instead in `is_eligible`.
        let extends = extends_names(self.file, decl);
        let fields = collect_fields(self.file, &decl.body.body);
        if extends.is_empty() && fields.len() < MIN_FIELDS {
            return;
        }
        self.shapes.push(TypeShape {
            name: decl.id.name.to_string(),
            file: self.file.path.clone(),
            span: span_from_bytes(&self.file.text, decl.id.span.start, decl.id.span.end),
            fields,
            extends,
        });
    }

    fn record_type_alias(&mut self, decl: &TSTypeAliasDeclaration<'a>) {
        let TSType::TSTypeLiteral(lit) = &decl.type_annotation else {
            return;
        };
        let fields = collect_fields(self.file, &lit.members);
        if fields.len() < MIN_FIELDS {
            return;
        }
        self.shapes.push(TypeShape {
            name: decl.id.name.to_string(),
            file: self.file.path.clone(),
            span: span_from_bytes(&self.file.text, decl.id.span.start, decl.id.span.end),
            fields,
            extends: Vec::new(),
        });
    }
}

#[cfg(test)]
mod differential {
    use super::*;
    use std::collections::HashSet;

    /// The pre-CD-180 implementation, kept verbatim as the reference the
    /// optimized `similarity` is differentially tested against. Do NOT
    /// "optimize" this copy — its whole value is being the naive
    /// definition.
    fn similarity_bruteforce(a: &TypeShape, b: &TypeShape) -> f64 {
        if a.extends != b.extends {
            return 0.0;
        }
        let names: HashSet<&String> = a.fields.keys().chain(b.fields.keys()).collect();
        if names.is_empty() {
            return if a.extends.is_empty() { 0.0 } else { 1.0 };
        }
        let matching = names
            .iter()
            .filter(|name| {
                a.fields
                    .get(**name)
                    .zip(b.fields.get(**name))
                    .is_some_and(|(x, y)| x == y)
            })
            .count();
        matching as f64 / names.len() as f64
    }

    /// The pre-CD-180 all-pairs clustering loop, kept as the reference
    /// for the bucketed/banded `compute_duplicates`. It shares
    /// `build_issues` with the optimized path, so the differential test
    /// isolates candidate-pair selection — the part that could silently
    /// drop a real duplicate.
    #[allow(clippy::needless_range_loop)]
    fn compute_duplicates_bruteforce(shapes: &[TypeShape]) -> Vec<Issue> {
        let n = shapes.len();
        let mut dsu = Dsu::new(n);
        for (i, shape_i) in shapes.iter().enumerate() {
            if !is_eligible(shape_i) {
                continue;
            }
            for j in (i + 1)..n {
                if !is_eligible(&shapes[j]) {
                    continue;
                }
                if similarity_bruteforce(shape_i, &shapes[j]) >= SIMILARITY_THRESHOLD {
                    dsu.union(i, j);
                }
            }
        }
        build_issues(shapes, &mut dsu)
    }

    /// xorshift64* — a fixed-seed PRNG so every trial is reproducible.
    /// This workspace has no `rand` dependency, and an unseeded generator
    /// would make a failing trial impossible to replay.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }
    }

    fn shape(
        name: &str,
        order: usize,
        fields: BTreeMap<String, String>,
        extends: Vec<String>,
    ) -> TypeShape {
        TypeShape {
            name: name.to_string(),
            file: PathBuf::from(format!("/p/f{}.ts", order % 7)),
            span: Span {
                start_byte: order as u32 * 10,
                end_byte: order as u32 * 10 + 5,
                line: order as u32 + 1,
                column: 1,
            },
            fields,
            extends,
        }
    }

    /// Field names are drawn from a small shared pool so shapes overlap
    /// often enough to actually exercise the near-threshold boundary, and
    /// type texts from an even smaller pool so same-name/different-type
    /// fields (which count toward the union but not toward `matching`)
    /// are common. `extends` is drawn from a pool containing the empty
    /// vector, a shared base, a disjoint base and a two-element base, so
    /// both bucket-splitting and the zero-field shared-base path (CD-135)
    /// get hit.
    fn random_shapes(rng: &mut Rng, count: usize) -> Vec<TypeShape> {
        const NAMES: &[&str] = &[
            "id",
            "name",
            "email",
            "age",
            "role",
            "createdAt",
            "updatedAt",
            "tags",
        ];
        const TYPES: &[&str] = &["string", "number", "boolean"];
        let extends_pool: Vec<Vec<String>> = vec![
            Vec::new(),
            vec!["Base".to_string()],
            vec!["Other".to_string()],
            vec!["Base".to_string(), "Mixin".to_string()],
        ];

        (0..count)
            .map(|i| {
                // 0..=NAMES.len() draws, so zero-field shapes (the 0/0
                // union edge case) and full-width shapes both occur.
                let draws = rng.below(NAMES.len() + 1);
                let mut fields = BTreeMap::new();
                for _ in 0..draws {
                    fields.insert(
                        NAMES[rng.below(NAMES.len())].to_string(),
                        TYPES[rng.below(TYPES.len())].to_string(),
                    );
                }
                let extends = extends_pool[rng.below(extends_pool.len())].clone();
                shape(&format!("T{i}"), i, fields, extends)
            })
            .collect()
    }

    fn fingerprint(issues: &[Issue]) -> Vec<String> {
        issues.iter().map(|i| format!("{i:?}")).collect()
    }

    fn sort_like_finalize(shapes: &mut [TypeShape]) {
        shapes.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
        });
    }

    #[test]
    fn optimized_similarity_matches_bruteforce() {
        let mut rng = Rng(0x5EED_1234_ABCD_0001);
        for trial in 0..2000 {
            let shapes = random_shapes(&mut rng, 6);
            for a in &shapes {
                for b in &shapes {
                    assert_eq!(
                        similarity(a, b),
                        similarity_bruteforce(a, b),
                        "trial {trial}: similarity mismatch for {:?}/{:?} vs {:?}/{:?}",
                        a.fields,
                        a.extends,
                        b.fields,
                        b.extends,
                    );
                }
            }
        }
    }

    #[test]
    fn optimized_compute_duplicates_matches_bruteforce() {
        let mut rng = Rng(0x5EED_1234_ABCD_0002);
        for trial in 0..1500 {
            let count = 1 + rng.below(40);
            let mut shapes = random_shapes(&mut rng, count);
            sort_like_finalize(&mut shapes);
            let expected = fingerprint(&compute_duplicates_bruteforce(&shapes));
            let actual = fingerprint(&compute_duplicates(&shapes));
            assert_eq!(
                expected,
                actual,
                "trial {trial}: output differs for {:?}",
                shapes
                    .iter()
                    .map(|s| (s.name.clone(), s.fields.clone(), s.extends.clone()))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// The randomized pool is deliberately narrow; this pins the exact
    /// ratio boundary the band filter is derived from: a smaller shape
    /// whose fields are a strict subset of a larger one, sized so the
    /// true similarity lands exactly on (or just under/over)
    /// `SIMILARITY_THRESHOLD`.
    #[test]
    fn band_filter_is_lossless_at_the_ratio_boundary() {
        for n in 3..60usize {
            for larger in n..=(n * 2 + 2) {
                let subset: BTreeMap<String, String> = (0..n)
                    .map(|k| (format!("f{k}"), "string".to_string()))
                    .collect();
                let superset: BTreeMap<String, String> = (0..larger)
                    .map(|k| (format!("f{k}"), "string".to_string()))
                    .collect();
                let shapes = vec![
                    shape("A", 0, subset, Vec::new()),
                    shape("B", 1, superset, Vec::new()),
                ];
                assert_eq!(
                    fingerprint(&compute_duplicates(&shapes)),
                    fingerprint(&compute_duplicates_bruteforce(&shapes)),
                    "boundary pair {n} vs {larger} handled differently"
                );
            }
        }
    }

    /// Zero-field shapes are only eligible via a non-empty `extends`
    /// (CD-135) and sit at the `0/0` end of the band derivation, so they
    /// get their own targeted case rather than relying on the randomized
    /// draw hitting them.
    #[test]
    fn zero_field_shared_base_shapes_still_cluster() {
        let base = vec!["Base".to_string()];
        let mut shapes = vec![
            shape("A", 0, BTreeMap::new(), base.clone()),
            shape("B", 1, BTreeMap::new(), base.clone()),
            shape("C", 2, BTreeMap::new(), vec!["Other".to_string()]),
            shape(
                "D",
                3,
                (0..4)
                    .map(|k| (format!("f{k}"), "string".to_string()))
                    .collect(),
                base,
            ),
        ];
        sort_like_finalize(&mut shapes);
        let issues = compute_duplicates(&shapes);
        assert_eq!(
            fingerprint(&issues),
            fingerprint(&compute_duplicates_bruteforce(&shapes))
        );
        assert_eq!(issues.len(), 1, "expected A/B to cluster alone: {issues:?}");
        assert!(issues[0].message.contains("extends the same base"));
    }
}
