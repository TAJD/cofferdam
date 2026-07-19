use std::collections::{BTreeMap, HashSet};
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
/// Comparison is pairwise (O(n^2) over collected shapes) but mutually-
/// similar shapes are transitively clustered via union-find: three
/// mutually-similar types produce one finding naming all three, not
/// three separate pairwise findings.
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
fn similarity(a: &TypeShape, b: &TypeShape) -> f64 {
    if a.extends != b.extends {
        return 0.0;
    }
    let names: HashSet<&String> = a.fields.keys().chain(b.fields.keys()).collect();
    if names.is_empty() {
        // Both sides declare no fields of their own. If they also share
        // a non-empty extends clause, that shared base alone is a full
        // duplication signal (CD-135); otherwise there's nothing to
        // compare.
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

// `j` indexes both `shapes` and `dsu`, so `enumerate()` (which only
// tracks one of the two) doesn't simplify this inner loop.
#[allow(clippy::needless_range_loop)]
fn compute_duplicates(shapes: &[TypeShape]) -> Vec<Issue> {
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
            if similarity(shape_i, &shapes[j]) >= SIMILARITY_THRESHOLD {
                dsu.union(i, j);
            }
        }
    }

    // Group eligible indices by DSU root; `shapes` is already sorted by
    // (file, span), so each group's members come out in that order and
    // the first member is a stable, deterministic anchor.
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, shape) in shapes.iter().enumerate() {
        if is_eligible(shape) {
            groups.entry(dsu.find(i)).or_default().push(i);
        }
    }

    let mut issues = Vec::new();
    for members in groups.into_values() {
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
