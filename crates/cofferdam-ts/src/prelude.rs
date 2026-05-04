//! Common imports for built-in and plugin TS checks.
//!
//! Built-in checks under `cofferdam-checks` and any out-of-tree plugin
//! crate that wires into the engine should `use cofferdam_ts::prelude::*;`
//! to get the standard scaffolding without naming every oxc symbol
//! individually. The contents are intentionally minimal — anything not
//! re-exported here is still reachable via the qualified
//! `cofferdam_ts::oxc_ast::ast::Foo` path.
//!
//! See `CLAUDE.md`'s "Required scaffolding for any AST check" section
//! for the canonical recipe.

pub use crate::ast::{AstView, AstVisitor, NodeKind, NodeRef, Walk};
pub use crate::language::TypeScript;
pub use crate::parser::{parse_into, source_type_for, ParsedView};
pub use crate::{Check, CheckContext, DynCheck, TsCheck};
pub use cofferdam_core::{
    span_from_bytes, Category, CheckMeta, CorpusIndex, CorpusKey, FileScope, FinalizeContext,
    Issue, Language, LineView, Lines, ParseOutcome, Priority, RelatedSpan, Severity, SourceFile,
    Span, TextEdit,
};
pub use oxc_ast_visit::{walk, Visit};
pub use oxc_syntax::scope::ScopeFlags;
