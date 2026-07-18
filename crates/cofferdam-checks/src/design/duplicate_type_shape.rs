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
/// type host this check doesn't require.
#[derive(Clone)]
struct TypeShape {
    name: String,
    file: PathBuf,
    span: Span,
    fields: BTreeMap<String, String>,
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
/// pairs of interfaces/type-literal aliases with sufficiently similar
/// field shapes.
///
/// Scope (v1): only `interface Foo { ... }` and `type Foo = { ... }`
/// declarations are considered — mapped types, unions, and other TSType
/// forms are not. An interface with a non-empty `extends` clause is
/// skipped, since its effective shape includes inherited fields this
/// check doesn't resolve, and comparing only the declared body would
/// risk false positives/negatives. Only plain identifier property keys
/// are counted; computed/private keys are ignored, so a shape with only
/// such keys will never reach `MIN_FIELDS`. Comparison is pairwise
/// (O(n^2) over collected shapes) and not transitively clustered — three
/// mutually-similar types produce three separate findings rather than
/// one group.
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

fn similarity(a: &TypeShape, b: &TypeShape) -> f64 {
    let names: HashSet<&String> = a.fields.keys().chain(b.fields.keys()).collect();
    if names.is_empty() {
        return 0.0;
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

fn compute_duplicates(shapes: &[TypeShape]) -> Vec<Issue> {
    let mut issues = Vec::new();
    for i in 0..shapes.len() {
        if shapes[i].fields.len() < MIN_FIELDS {
            continue;
        }
        for j in (i + 1)..shapes.len() {
            if shapes[j].fields.len() < MIN_FIELDS {
                continue;
            }
            let score = similarity(&shapes[i], &shapes[j]);
            if score < SIMILARITY_THRESHOLD {
                continue;
            }
            let a = &shapes[i];
            let b = &shapes[j];
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "`{}` ({} fields) is {}% structurally similar to `{}` in {} — consider a shared type",
                    a.name,
                    a.fields.len(),
                    (score * 100.0).round() as i64,
                    b.name,
                    b.file.display()
                ),
                file: a.file.clone(),
                location: Location::from_span(&a.file, a.span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: vec![RelatedSpan {
                    file: b.file.clone(),
                    location: Location::from_span(&b.file, b.span),
                }],
            });
        }
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

impl<'a> ShapeCollector<'a> {
    fn record_interface(&mut self, decl: &TSInterfaceDeclaration<'a>) {
        // Inherited fields from `extends` aren't visible in the body, so
        // comparing only the declared members here would risk false
        // positives/negatives against the interface's real shape — skip.
        if !decl.extends.is_empty() {
            return;
        }
        let fields = collect_fields(self.file, &decl.body.body);
        if fields.len() < MIN_FIELDS {
            return;
        }
        self.shapes.push(TypeShape {
            name: decl.id.name.to_string(),
            file: self.file.path.clone(),
            span: span_from_bytes(&self.file.text, decl.id.span.start, decl.id.span.end),
            fields,
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
        });
    }
}
