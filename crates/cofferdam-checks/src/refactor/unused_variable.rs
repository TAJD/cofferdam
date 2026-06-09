use std::collections::HashSet;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{
    BindingRestElement, Class, ComputedMemberExpression, Expression, FormalParameter, Function,
    StaticMemberExpression,
};
use oxc_ast_visit::Visit;
use oxc_semantic::SemanticBuilder;
use oxc_syntax::symbol::SymbolFlags;

// ─── Refactor.UnusedVariable ───────────────────────────────────────────────
//
// Flag local-scope bindings (let/const/var/function/class/parameters/catch
// variables) that are declared but never read. Built on top of
// `oxc_semantic`, which gives us proper scope and reference tracking out of
// the box — rolling our own scope walker would be a large yak-shave with a
// high false-positive risk, and false positives on "unused" murder this
// check's credibility.
//
// ## Filters (in skip order)
//
// - **Type-only symbols** (TypeAlias, Interface, Enum, TypeParameter,
//   TypeImport, NamespaceModule). Out of scope per cd-ydw — needs the
//   type-aware tier (phase 5).
// - **ESM imports** (Import). Tree-shaker territory, not us.
// - **Underscore prefix** (`_unused`). Universal opt-out convention; respect
//   it without a config knob.
// - **Rest patterns** (`const [a, ...rest] = arr` / `function (...args)`).
//   Conventionally always considered used — flagging them produces noise on
//   every codebase that uses `const [, ...rest]` for "skip first" semantics.
// - **Resolved references non-empty.** The actual "is it used" signal.
//
// ## Position-of-parameter false-positives
//
// A common ESLint-no-unused-vars footgun: `function f(a, b) { return b; }`
// flags `a` as unused, but you can't remove it without breaking the
// signature. We rely on the `_` prefix convention to opt out (`_a`) rather
// than implementing "after-used" semantics in v1. Document the convention
// in the message so the fix is obvious. Revisit once we have telemetry on
// real-world false-positive rates.
//
// ## Module-scope bindings
//
// Top-level (Program-scope) bindings might be exported, and oxc's semantic
// model doesn't count export specifiers as "uses". Without walking the
// export AST to enumerate exported names, we can't tell `export const foo`
// from a real unused module-level binding — so v1 takes the conservative
// line and skips Program-scope symbols entirely. The trade-off: a
// top-level non-exported `const FOO = ...` that's truly unused won't
// flag. Acceptable v1 cut; the alternative is false positives on every
// `export const` in the codebase, which would torch the check's
// credibility on day one.

/// `Refactor.UnusedVariable` — flags `let` / `const` declarations
/// whose binding is never read in the same scope. See `CheckMeta`
/// for the module-scope export carve-out.
pub struct UnusedVariable;

const UNUSED_META: CheckMeta = CheckMeta {
    id: "Refactor.UnusedVariable",
    category: Category::Refactor,
    base_priority: 10,
    default_severity: Severity::Low,
    explanation: "Variables declared but never read are dead code. Prefix with `_` to opt out where the binding is intentionally unused (e.g., positional function parameters).",
    body: include_str!("../../docs/Refactor.UnusedVariable.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

/// Bitmask of symbol kinds we'll flag when unused. Excludes type-only
/// symbols, imports, and module-level value bindings that are exported.
/// The exact filtering happens per-symbol below — this is the broad gate.
const FLAGGABLE_KINDS: SymbolFlags = SymbolFlags::Variable
    .union(SymbolFlags::CatchVariable)
    .union(SymbolFlags::Function)
    .union(SymbolFlags::Class);

/// Symbol flags that disqualify a binding from the unused check, even if
/// it has zero references. Type-only stuff defers to phase 5; imports
/// belong to the tree-shaker; ambient declarations (`declare ...`) are
/// type-system signals not real bindings.
const SKIP_KINDS: SymbolFlags = SymbolFlags::TypeAlias
    .union(SymbolFlags::Interface)
    .union(SymbolFlags::Enum)
    .union(SymbolFlags::EnumMember)
    .union(SymbolFlags::TypeParameter)
    .union(SymbolFlags::TypeImport)
    .union(SymbolFlags::Import)
    .union(SymbolFlags::NamespaceModule)
    .union(SymbolFlags::ValueModule)
    .union(SymbolFlags::Ambient);

impl Check for UnusedVariable {
    fn meta(&self) -> &'static CheckMeta {
        &UNUSED_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };

        // Collect rest-pattern spans up front. Symbols whose declaration
        // sits inside any rest span are intentional discards and must not
        // flag.
        //
        // Same pre-walk also collects TypeScript function-signature
        // spans (TSFunctionType, TSMethodSignature, TSCallSignature,
        // TSConstructSignature, TSIndexSignature). Parameters declared
        // there exist purely for documentation/IDE tooltips — they're
        // type-position, not value-position bindings, even though oxc's
        // semantic builder still tracks them as FunctionScopedVariable.
        let mut skip_collector = SkipSpanCollector {
            rest_ranges: Vec::new(),
            ts_signature_ranges: Vec::new(),
            named_expression_id_ranges: Vec::new(),
            param_property_ranges: Vec::new(),
            class_stack: Vec::new(),
            class_this_reads: Vec::new(),
        };
        skip_collector.visit_program(parsed.program);
        let rest_ranges = skip_collector.rest_ranges;
        let ts_signature_ranges = skip_collector.ts_signature_ranges;
        let named_expr_ranges = skip_collector.named_expression_id_ranges;
        let param_property_ranges = skip_collector.param_property_ranges;
        let class_this_reads = skip_collector.class_this_reads;

        let semantic_return = SemanticBuilder::new().build(parsed.program);
        let scoping = semantic_return.semantic.scoping();
        let root_scope = scoping.root_scope_id();

        let mut issues = Vec::new();
        for symbol_id in scoping.symbol_ids() {
            let flags = scoping.symbol_flags(symbol_id);

            if flags.intersects(SKIP_KINDS) {
                continue;
            }
            if !flags.intersects(FLAGGABLE_KINDS) {
                continue;
            }
            // Module-scope (program-level) bindings may be exported —
            // see the file-header note above. Skip them in v1.
            if scoping.symbol_scope_id(symbol_id) == root_scope {
                continue;
            }

            let name = scoping.symbol_name(symbol_id);
            if name.starts_with('_') {
                continue;
            }

            let symbol_span = scoping.symbol_span(symbol_id);
            if span_in_any_range(symbol_span.start, symbol_span.end, &rest_ranges) {
                continue;
            }
            // TS type-position function signatures: parameter names
            // there are documentation, not real bindings.
            if span_in_any_range(symbol_span.start, symbol_span.end, &ts_signature_ranges) {
                continue;
            }
            // Named function/class expressions: the inner name exists
            // for self-reference + stack traces, not for outside use.
            if span_in_any_range(symbol_span.start, symbol_span.end, &named_expr_ranges) {
                continue;
            }

            // TS parameter property read via `this.<name>` (cd-sh72 / gh
            // #44). `constructor(private ctx: T)` is both a parameter and
            // a class field; oxc resolves `this.ctx` as a member access,
            // not a reference to the parameter symbol, so
            // get_resolved_references is empty even when the field is
            // used. Treat a `this.<name>` read in the enclosing class as
            // a use. A parameter property never read this way still falls
            // through and flags (it has no resolved references either).
            if span_in_any_range(symbol_span.start, symbol_span.end, &param_property_ranges)
                && class_reads_this_name(
                    symbol_span.start,
                    symbol_span.end,
                    name,
                    &class_this_reads,
                )
            {
                continue;
            }

            // The actual "is it used" signal.
            if scoping.get_resolved_references(symbol_id).next().is_some() {
                continue;
            }

            let span = span_from_bytes(&file.text, symbol_span.start, symbol_span.end);
            issues.push(Issue {
                check_id: UNUSED_META.id.to_string(),
                message: format!(
                    "`{name}` is declared but never read (prefix with `_` if intentional)"
                ),
                file: file.path.clone(),
                location: Location::from_span(&file.path, span),
                priority: Priority(UNUSED_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }

        issues
    }
}

struct SkipSpanCollector {
    rest_ranges: Vec<(u32, u32)>,
    ts_signature_ranges: Vec<(u32, u32)>,
    /// Spans of binding identifiers that name a function expression or
    /// class expression. These names exist for self-reference + clearer
    /// stack traces (a common pattern in `React.forwardRef(function
    /// Foo() {})` and named class-expression mocks); they're not really
    /// "unused" in any actionable sense if the expression itself flows
    /// somewhere via assignment.
    named_expression_id_ranges: Vec<(u32, u32)>,
    /// Binding-identifier spans of TS parameter properties
    /// (`constructor(private ctx: T)`). A parameter property declares a
    /// class field read via `this.<name>` — which oxc resolves as a
    /// member access, NOT a reference to the parameter symbol — so the
    /// plain reference signal misses the read (cd-sh72 / gh #44).
    param_property_ranges: Vec<(u32, u32)>,
    /// In-progress stack of enclosing class spans + the `this.<name>`
    /// member names read inside each. Pushed on class entry, popped into
    /// `class_this_reads` on exit so the innermost class wins.
    class_stack: Vec<ClassThisReads>,
    /// Finalised per-class `this.<name>` read sets.
    class_this_reads: Vec<ClassThisReads>,
}

/// A class span paired with the set of `this.<name>` member names read
/// anywhere inside it. Used to decide whether a parameter property is
/// actually live (`this.ctx`) versus genuinely unused.
#[derive(Clone)]
struct ClassThisReads {
    span: (u32, u32),
    names: HashSet<String>,
}

impl<'a> Visit<'a> for SkipSpanCollector {
    fn visit_binding_rest_element(&mut self, node: &BindingRestElement<'a>) {
        self.rest_ranges.push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_binding_rest_element(self, node);
    }

    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        // Function expressions only — declarations bind into the
        // surrounding scope and *should* flag if unused.
        if matches!(
            node.r#type,
            oxc_ast::ast::FunctionType::FunctionExpression
                | oxc_ast::ast::FunctionType::TSEmptyBodyFunctionExpression
        ) {
            if let Some(id) = node.id.as_ref() {
                self.named_expression_id_ranges
                    .push((id.span.start, id.span.end));
            }
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_class(&mut self, node: &Class<'a>) {
        if matches!(node.r#type, oxc_ast::ast::ClassType::ClassExpression) {
            if let Some(id) = node.id.as_ref() {
                self.named_expression_id_ranges
                    .push((id.span.start, id.span.end));
            }
        }
        // Track this class while we walk it so `this.<name>` reads inside
        // its methods attribute to it (innermost class wins). Popped and
        // recorded on exit.
        self.class_stack.push(ClassThisReads {
            span: (node.span.start, node.span.end),
            names: HashSet::new(),
        });
        oxc_ast_visit::walk::walk_class(self, node);
        if let Some(done) = self.class_stack.pop() {
            self.class_this_reads.push(done);
        }
    }

    fn visit_formal_parameter(&mut self, node: &FormalParameter<'a>) {
        // A constructor parameter carrying an access modifier
        // (`public`/`private`/`protected`) or `readonly` is a TS
        // parameter property — it declares a class field, not just a
        // positional argument. The parser only permits these modifiers on
        // constructor params, so the modifier presence is a sufficient
        // signal. Parameter properties are always simple identifiers
        // (no destructuring), so the binding is a BindingIdentifier.
        if node.accessibility.is_some() || node.readonly {
            if let Some(id) = node.pattern.get_binding_identifier() {
                self.param_property_ranges
                    .push((id.span.start, id.span.end));
            }
        }
        oxc_ast_visit::walk::walk_formal_parameter(self, node);
    }

    fn visit_static_member_expression(&mut self, node: &StaticMemberExpression<'a>) {
        // `this.ctx` — record `ctx` against the innermost enclosing class.
        // For `this.ctx.value` the outer member's object is the inner
        // `this.ctx` member (not `this`), so walking records `ctx` from
        // the inner node and ignores `value` — exactly the field name we
        // want.
        if matches!(node.object, Expression::ThisExpression(_)) {
            if let Some(top) = self.class_stack.last_mut() {
                top.names.insert(node.property.name.as_str().to_string());
            }
        }
        oxc_ast_visit::walk::walk_static_member_expression(self, node);
    }

    fn visit_computed_member_expression(&mut self, node: &ComputedMemberExpression<'a>) {
        // `this['ctx']` with a string-literal key — same field read,
        // different syntax. A dynamic key (`this[k]`) can't be resolved
        // statically; a parameter property reachable only that way is a
        // rare, accepted false-positive gap.
        if matches!(node.object, Expression::ThisExpression(_)) {
            if let Expression::StringLiteral(s) = &node.expression {
                if let Some(top) = self.class_stack.last_mut() {
                    top.names.insert(s.value.as_str().to_string());
                }
            }
        }
        oxc_ast_visit::walk::walk_computed_member_expression(self, node);
    }

    fn visit_ts_function_type(&mut self, node: &oxc_ast::ast::TSFunctionType<'a>) {
        self.ts_signature_ranges
            .push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_ts_function_type(self, node);
    }

    fn visit_ts_method_signature(&mut self, node: &oxc_ast::ast::TSMethodSignature<'a>) {
        self.ts_signature_ranges
            .push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_ts_method_signature(self, node);
    }

    fn visit_ts_call_signature_declaration(
        &mut self,
        node: &oxc_ast::ast::TSCallSignatureDeclaration<'a>,
    ) {
        self.ts_signature_ranges
            .push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_ts_call_signature_declaration(self, node);
    }

    fn visit_ts_construct_signature_declaration(
        &mut self,
        node: &oxc_ast::ast::TSConstructSignatureDeclaration<'a>,
    ) {
        self.ts_signature_ranges
            .push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_ts_construct_signature_declaration(self, node);
    }

    fn visit_ts_index_signature(&mut self, node: &oxc_ast::ast::TSIndexSignature<'a>) {
        self.ts_signature_ranges
            .push((node.span.start, node.span.end));
        oxc_ast_visit::walk::walk_ts_index_signature(self, node);
    }
}

/// True iff `[start, end)` is fully contained within any of the given
/// half-open ranges. Used to detect whether a symbol's declaration site
/// sits inside a rest pattern.
fn span_in_any_range(start: u32, end: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|&(rs, re)| start >= rs && end <= re)
}

/// True iff the innermost class enclosing `[start, end)` reads a
/// `this.<name>` member named `name`. Lets a parameter property accessed
/// through `this` count as a live read even though oxc doesn't resolve
/// the member access back to the parameter symbol. Scoped per class (the
/// innermost enclosing one) so the same field name in a sibling class
/// that never reads it still flags.
fn class_reads_this_name(start: u32, end: u32, name: &str, classes: &[ClassThisReads]) -> bool {
    classes
        .iter()
        .filter(|c| start >= c.span.0 && end <= c.span.1)
        .max_by_key(|c| c.span.0)
        .is_some_and(|c| c.names.contains(name))
}
