//! `cofferdam-ts` — TypeScript language adapter for the cofferdam
//! analyzer.
//!
//! Wraps the oxc parser and the AST surface that built-in checks
//! (and, via `cofferdam-napi`, plugin checks) operate against.
//! Implements `cofferdam_core::Language` for `TypeScript`, so the
//! engine + Check trait (which live in `cofferdam-core`) are generic
//! over the language and a future `cofferdam-py` adapter can slot in
//! alongside without touching the platform crates.
//!
//! See `design/platform-extensibility.md` for the architectural split.

pub mod ast;
pub mod language;
pub mod line_classify;
pub mod parser;
pub mod prelude;

pub use ast::{AstView, AstVisitor, NodeKind, NodeRef, Walk};
pub use language::TypeScript;
pub use line_classify::build_lines;
pub use parser::{diagnostic_messages, parse_fatal, parse_into, source_type_for, ParsedView};

/// Re-export of the generic `Check` trait from `cofferdam-core`.
/// Built-in TS checks impl `Check<TypeScript> for X`. The trait alias
/// `TsCheck` below saves the `<TypeScript>` boilerplate on bounds.
pub use cofferdam_core::Check;
/// `CheckContext` bound to the TypeScript adapter — what every
/// built-in check sees in its `run`/`pass2` parameter. The two
/// lifetimes (`'p` parsed-arena, `'r` run-scoped) are usually elided
/// in check signatures via `&mut CheckContext<'_, '_>`.
pub type CheckContext<'p, 'r> = cofferdam_core::CheckContext<'p, 'r, TypeScript>;
/// `dyn`-form of `Check<TypeScript>` for `Box<DynCheck>` / `&DynCheck`
/// containers (the engine's check vector, the CLI's check map).
pub type DynCheck = dyn cofferdam_core::Check<TypeScript>;

// Re-export the oxc bits that plugin / built-in check authors will
// reach for. Going through `cofferdam_ts::oxc_*` (rather than depending
// on `oxc_*` directly) is what keeps the "no oxc outside the adapter"
// CI guardrail meaningful — only this crate is allowed to import oxc.
pub use oxc_allocator;
pub use oxc_allocator::Allocator;
pub use oxc_ast;
pub use oxc_ast_visit;
pub use oxc_diagnostics;
pub use oxc_parser;
pub use oxc_semantic;
pub use oxc_span;
pub use oxc_syntax;

/// Marker trait for "any `Check<TypeScript>`" — usable in trait
/// bounds (`T: TsCheck`) without re-spelling the language parameter.
/// Auto-implemented; no manual `impl TsCheck` needed.
pub trait TsCheck: cofferdam_core::Check<TypeScript> {}
impl<T: cofferdam_core::Check<TypeScript> + ?Sized> TsCheck for T {}

/// Extension trait that adds `ast()` to a TS-flavored `CheckContext`.
/// Lives here (not in core) because `AstView` is a TS-specific surface.
pub trait CheckContextExt<'p> {
    /// Plugin-facing AST surface. `None` when the file failed to parse
    /// (engine emitted `Warning.ParseError` for those). Built-in checks
    /// may continue to use `ctx.parsed` directly with `oxc_ast_visit`;
    /// this method is the layered, stable surface used by plugins.
    fn ast(&self) -> Option<AstView<'p>>;
}

impl<'p, 'r: 'p> CheckContextExt<'p> for cofferdam_core::CheckContext<'p, 'r, TypeScript> {
    fn ast(&self) -> Option<AstView<'p>> {
        self.parsed.map(|p| {
            // `&self.file.text` has lifetime `'r`; `'r: 'p` lets it
            // coerce to `&'p str` so AstView's two borrows agree on
            // the parse-arena lifetime.
            let text: &'p str = &self.file.text;
            AstView::new(p.program, text)
        })
    }
}
