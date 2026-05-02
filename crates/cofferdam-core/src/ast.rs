//! Plugin-facing AST surface — the ergonomic wrapper that custom-check
//! authors program against, layered over `oxc_ast` + `oxc_ast_visit`.
//!
//! Three access patterns, mapped from rovikore-host's existing 11 custom
//! Credo checks (see `rovikore_host_credo_checks` memory file):
//!
//! 1. **Eager filter** — `view.find_all(NodeKind::CallExpression)` returns
//!    every node of a kind. Mirrors Credo's `prewalk` with clause-head
//!    pattern matching. NoHttpClient lives here.
//! 2. **Visitor with state** — `view.walk(&mut visitor)` runs an
//!    [`AstVisitor`] over the tree. Each `visit_*` returns [`Walk`] —
//!    `Continue` descends, `Skip` halts descent into this node only.
//!    Mirrors Credo's stateful `prewalk` accumulator. TenantIsolation
//!    lives here.
//! 3. **Direct rooted access** — `view.root()` returns `&Program<'a>` for
//!    surgical inspection. Escape hatch when the kinds we expose don't
//!    cover the case yet.
//!
//! Built-in checks may keep using `oxc_ast_visit::Visit` directly — this
//! surface is the *plugin* contract, designed so the napi proxy in
//! cd-81a.7 can mirror it 1:1 in TypeScript without leaking oxc internals.
//!
//! # Spans
//!
//! Every node we re-expose has a `span()` accessor returning `oxc_span::Span`
//! (byte offsets). Use [`AstView::span_of`] to convert to our
//! line/column [`Span`] — that is the only correct conversion path
//! (UTF-8 column nuance, see [`crate::span_util::span_from_bytes`]).

use oxc_ast::ast::{
    ArrowFunctionExpression, BinaryExpression, CallExpression, Class, Function,
    IdentifierReference, ImportDeclaration, MemberExpression, NewExpression, NumericLiteral,
    ObjectExpression, Program, StringLiteral,
};
use oxc_ast_visit::{walk, Visit};
use oxc_syntax::scope::ScopeFlags;

use crate::issue::Span;
use crate::span_util::span_from_bytes;

/// Borrowed view of a parsed file's AST. Cheap to construct; clones share
/// the same underlying borrow.
#[derive(Clone, Copy)]
pub struct AstView<'a> {
    program: &'a Program<'a>,
    text: &'a str,
}

impl<'a> AstView<'a> {
    pub fn new(program: &'a Program<'a>, text: &'a str) -> Self {
        Self { program, text }
    }

    /// Direct access to the oxc `Program` root.
    pub fn root(&self) -> &'a Program<'a> {
        self.program
    }

    /// Source text the AST was parsed from. Useful for slicing node
    /// source via `&text[span.start as usize..span.end as usize]`.
    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Convert oxc byte offsets into our line/column-aware [`Span`].
    pub fn span_of(&self, start_byte: u32, end_byte: u32) -> Span {
        span_from_bytes(self.text, start_byte, end_byte)
    }

    /// Eagerly collect every node of `kind` in document order.
    ///
    /// O(N) over the AST. Allocates a Vec. For multi-kind queries
    /// prefer a single [`walk`](Self::walk) pass.
    pub fn find_all(&self, kind: NodeKind) -> Vec<NodeRef<'a>> {
        let mut collector = FindAllCollector {
            kind,
            found: Vec::new(),
        };
        collector.visit_program(self.program);
        collector.found
    }

    /// Run a stateful visitor over the tree.
    ///
    /// Each `visit_*` method on the visitor returns [`Walk`]:
    /// `Walk::Continue` descends into the node's children; `Walk::Skip`
    /// stops descent into that node only (sibling nodes still visit).
    pub fn walk<V: AstVisitor<'a>>(&self, visitor: &mut V) {
        let mut adapter = WalkAdapter { inner: visitor };
        adapter.visit_program(self.program);
    }
}

/// Outcome of an [`AstVisitor`] visit. Controls per-node descent.
///
/// Whole-tree early-termination is not exposed — kept out deliberately
/// to keep the surface minimal (cd-81a.2 acceptance is "stop descent",
/// singular). Authors who need it use [`AstView::root`] and write their
/// own walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Descend into this node's children.
    Continue,
    /// Skip this node's children. Continue with the next sibling.
    Skip,
}

/// Stateful visitor for [`AstView::walk`]. Override only the kinds you
/// care about; defaults are `Walk::Continue` no-ops.
///
/// `ScopeFlags` and other oxc internals are deliberately hidden — this
/// is the plugin-author surface, kept stable while oxc evolves.
#[allow(unused_variables)]
pub trait AstVisitor<'a> {
    fn visit_call_expression(&mut self, node: &'a CallExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_new_expression(&mut self, node: &'a NewExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_import_declaration(&mut self, node: &'a ImportDeclaration<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_function(&mut self, node: &'a Function<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_arrow_function_expression(&mut self, node: &'a ArrowFunctionExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_binary_expression(&mut self, node: &'a BinaryExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_class(&mut self, node: &'a Class<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_object_expression(&mut self, node: &'a ObjectExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_member_expression(&mut self, node: &'a MemberExpression<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_identifier_reference(&mut self, node: &'a IdentifierReference<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_string_literal(&mut self, node: &'a StringLiteral<'a>) -> Walk {
        Walk::Continue
    }
    fn visit_numeric_literal(&mut self, node: &'a NumericLiteral<'a>) -> Walk {
        Walk::Continue
    }
}

/// Kinds exposed via [`AstView::find_all`] and [`NodeRef`]. Deliberately
/// a small set covering the rovikore-host check patterns; grow as plugin
/// needs surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    CallExpression,
    NewExpression,
    ImportDeclaration,
    Function,
    ArrowFunctionExpression,
    BinaryExpression,
    Class,
    ObjectExpression,
    MemberExpression,
    IdentifierReference,
    StringLiteral,
    NumericLiteral,
}

/// Borrowed reference to a discovered node, tagged by kind. Pattern-match
/// to recover the typed `&'a Foo<'a>` for member access.
#[derive(Debug, Clone, Copy)]
pub enum NodeRef<'a> {
    CallExpression(&'a CallExpression<'a>),
    NewExpression(&'a NewExpression<'a>),
    ImportDeclaration(&'a ImportDeclaration<'a>),
    Function(&'a Function<'a>),
    ArrowFunctionExpression(&'a ArrowFunctionExpression<'a>),
    BinaryExpression(&'a BinaryExpression<'a>),
    Class(&'a Class<'a>),
    ObjectExpression(&'a ObjectExpression<'a>),
    MemberExpression(&'a MemberExpression<'a>),
    IdentifierReference(&'a IdentifierReference<'a>),
    StringLiteral(&'a StringLiteral<'a>),
    NumericLiteral(&'a NumericLiteral<'a>),
}

impl<'a> NodeRef<'a> {
    /// oxc byte-offset span. Convert with [`AstView::span_of`] for line/col.
    pub fn span(&self) -> oxc_span::Span {
        match self {
            Self::CallExpression(n) => n.span,
            Self::NewExpression(n) => n.span,
            Self::ImportDeclaration(n) => n.span,
            Self::Function(n) => n.span,
            Self::ArrowFunctionExpression(n) => n.span,
            Self::BinaryExpression(n) => n.span,
            Self::Class(n) => n.span,
            Self::ObjectExpression(n) => n.span,
            Self::MemberExpression(n) => match n {
                MemberExpression::ComputedMemberExpression(m) => m.span,
                MemberExpression::StaticMemberExpression(m) => m.span,
                MemberExpression::PrivateFieldExpression(m) => m.span,
            },
            Self::IdentifierReference(n) => n.span,
            Self::StringLiteral(n) => n.span,
            Self::NumericLiteral(n) => n.span,
        }
    }

    pub fn kind(&self) -> NodeKind {
        match self {
            Self::CallExpression(_) => NodeKind::CallExpression,
            Self::NewExpression(_) => NodeKind::NewExpression,
            Self::ImportDeclaration(_) => NodeKind::ImportDeclaration,
            Self::Function(_) => NodeKind::Function,
            Self::ArrowFunctionExpression(_) => NodeKind::ArrowFunctionExpression,
            Self::BinaryExpression(_) => NodeKind::BinaryExpression,
            Self::Class(_) => NodeKind::Class,
            Self::ObjectExpression(_) => NodeKind::ObjectExpression,
            Self::MemberExpression(_) => NodeKind::MemberExpression,
            Self::IdentifierReference(_) => NodeKind::IdentifierReference,
            Self::StringLiteral(_) => NodeKind::StringLiteral,
            Self::NumericLiteral(_) => NodeKind::NumericLiteral,
        }
    }
}

// ---- internal: find_all collector -----------------------------------

struct FindAllCollector<'a> {
    kind: NodeKind,
    found: Vec<NodeRef<'a>>,
}

// Single Visit impl that pushes nodes matching `self.kind`. Always
// descends — we want every match in the tree.
impl<'a> Visit<'a> for FindAllCollector<'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.kind == NodeKind::CallExpression {
            self.found.push(NodeRef::CallExpression(unsafe_extend(it)));
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        if self.kind == NodeKind::NewExpression {
            self.found.push(NodeRef::NewExpression(unsafe_extend(it)));
        }
        walk::walk_new_expression(self, it);
    }
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if self.kind == NodeKind::ImportDeclaration {
            self.found
                .push(NodeRef::ImportDeclaration(unsafe_extend(it)));
        }
        walk::walk_import_declaration(self, it);
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if self.kind == NodeKind::Function {
            self.found.push(NodeRef::Function(unsafe_extend(it)));
        }
        walk::walk_function(self, it, flags);
    }
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        if self.kind == NodeKind::ArrowFunctionExpression {
            self.found
                .push(NodeRef::ArrowFunctionExpression(unsafe_extend(it)));
        }
        walk::walk_arrow_function_expression(self, it);
    }
    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        if self.kind == NodeKind::BinaryExpression {
            self.found
                .push(NodeRef::BinaryExpression(unsafe_extend(it)));
        }
        walk::walk_binary_expression(self, it);
    }
    fn visit_class(&mut self, it: &Class<'a>) {
        if self.kind == NodeKind::Class {
            self.found.push(NodeRef::Class(unsafe_extend(it)));
        }
        walk::walk_class(self, it);
    }
    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        if self.kind == NodeKind::ObjectExpression {
            self.found
                .push(NodeRef::ObjectExpression(unsafe_extend(it)));
        }
        walk::walk_object_expression(self, it);
    }
    fn visit_member_expression(&mut self, it: &MemberExpression<'a>) {
        if self.kind == NodeKind::MemberExpression {
            self.found
                .push(NodeRef::MemberExpression(unsafe_extend(it)));
        }
        walk::walk_member_expression(self, it);
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.kind == NodeKind::IdentifierReference {
            self.found
                .push(NodeRef::IdentifierReference(unsafe_extend(it)));
        }
        walk::walk_identifier_reference(self, it);
    }
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if self.kind == NodeKind::StringLiteral {
            self.found.push(NodeRef::StringLiteral(unsafe_extend(it)));
        }
        walk::walk_string_literal(self, it);
    }
    fn visit_numeric_literal(&mut self, it: &NumericLiteral<'a>) {
        if self.kind == NodeKind::NumericLiteral {
            self.found.push(NodeRef::NumericLiteral(unsafe_extend(it)));
        }
        walk::walk_numeric_literal(self, it);
    }
}

// oxc's `Visit::visit_X(&mut self, it: &T<'a>)` gives us a `&'_ T<'a>` —
// the inner lifetime is `'a` (the arena), the outer is the visit-call's
// stack frame. The arena outlives the visitor, so re-borrowing the inner
// reference as `&'a T<'a>` is sound: the arena owns the data, and we
// only ever store it inside `Vec<NodeRef<'a>>` which is tied to the
// `AstView<'a>` borrow of the arena.
fn unsafe_extend<'a, T>(r: &T) -> &'a T {
    // Safety: see comment above. The arena owning `T` lives at least as
    // long as `'a`, the lifetime of the `AstView` we're walking.
    unsafe { &*(r as *const T) }
}

// ---- internal: walk adapter for AstVisitor + Walk ------------------

struct WalkAdapter<'v, V: ?Sized> {
    inner: &'v mut V,
}

impl<'a, 'v, V: AstVisitor<'a> + ?Sized> Visit<'a> for WalkAdapter<'v, V> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if matches!(
            self.inner.visit_call_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_call_expression(self, it);
        }
    }
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        if matches!(
            self.inner.visit_new_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_new_expression(self, it);
        }
    }
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if matches!(
            self.inner.visit_import_declaration(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_import_declaration(self, it);
        }
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if matches!(self.inner.visit_function(unsafe_extend(it)), Walk::Continue) {
            walk::walk_function(self, it, flags);
        }
    }
    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        if matches!(
            self.inner
                .visit_arrow_function_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_arrow_function_expression(self, it);
        }
    }
    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        if matches!(
            self.inner.visit_binary_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_binary_expression(self, it);
        }
    }
    fn visit_class(&mut self, it: &Class<'a>) {
        if matches!(self.inner.visit_class(unsafe_extend(it)), Walk::Continue) {
            walk::walk_class(self, it);
        }
    }
    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        if matches!(
            self.inner.visit_object_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_object_expression(self, it);
        }
    }
    fn visit_member_expression(&mut self, it: &MemberExpression<'a>) {
        if matches!(
            self.inner.visit_member_expression(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_member_expression(self, it);
        }
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if matches!(
            self.inner.visit_identifier_reference(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_identifier_reference(self, it);
        }
    }
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if matches!(
            self.inner.visit_string_literal(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_string_literal(self, it);
        }
    }
    fn visit_numeric_literal(&mut self, it: &NumericLiteral<'a>) {
        if matches!(
            self.inner.visit_numeric_literal(unsafe_extend(it)),
            Walk::Continue
        ) {
            walk::walk_numeric_literal(self, it);
        }
    }
}
