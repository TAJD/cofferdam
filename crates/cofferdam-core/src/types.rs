//! Type-aware check support (cd-9hp.2).
//!
//! TypeScript's type system lives in Node (via the TS Compiler API),
//! not in oxc — cofferdam's native parser is fast but type-free. Checks
//! that need resolved type information declare `CheckMeta::requires_types
//! = true`; the engine routes them through a [`TypeOracle`] backed by a
//! Node-side ts-morph worker (the type host, `cd-9hp.2`).
//!
//! This module defines only the *contract*: the trait checks call and
//! the compact facts they get back. The worker process, the JSON-RPC
//! wire, and the concrete oracle implementation all live in the CLI
//! (`crates/cofferdam-cli/src/type_host.rs`) so `cofferdam-core` stays
//! dependency-light and free of any Node coupling. Tests install a
//! pure-Rust stub oracle.

use std::path::Path;

/// Resolved type facts for one AST node, as reported by the type host.
///
/// Deliberately compact: type-aware checks ask narrow yes/no questions
/// ("can this be null?", "is this `any`?"), not "hand me the full type
/// graph". Serialising a whole TS type across the process boundary
/// would be slow and mostly wasted. New fields are added here as new
/// checks need them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFacts {
    /// The type's printed text, e.g. `"string | null"`. Human-facing —
    /// used in finding messages, never matched on programmatically
    /// (the boolean flags are the contract for logic).
    pub text: String,
    /// True when the type includes `null` or `undefined` — i.e. a
    /// nullish guard against a value of this type is meaningful.
    pub is_nullable: bool,
    /// True when the type's union includes `null`.
    pub includes_null: bool,
    /// True when the type's union includes `undefined`.
    pub includes_undefined: bool,
    /// True when the type is `any` or `unknown`. Type-narrowing checks
    /// MUST bail on this: the compiler can't prove a guard is
    /// redundant against a type it knows nothing about, so flagging
    /// would produce false positives.
    pub is_any: bool,
}

/// Read-only handle a type-aware check uses to ask the type host about
/// nodes in the file currently being analyzed.
///
/// Backed in production by a Node ts-morph worker
/// ([`crate`](crate)-external; see `cofferdam-cli`); tests install a
/// pure-Rust stub. The engine threads the active oracle (if any) into
/// every [`CheckContext`](crate::CheckContext) via
/// [`CheckContext::with_types`].
///
/// `Send + Sync` so the engine can hold one behind a shared reference
/// across the per-file loop (and, once per-file parallelism lands, share
/// it across threads — the worker-backed impl serialises requests behind
/// a mutex).
pub trait TypeOracle: Send + Sync {
    /// Resolve type facts for the node spanning `[start_byte, end_byte)`
    /// in `file`. Byte offsets are oxc UTF-8 offsets (the same ones
    /// carried by [`Span`](crate::Span)); the worker translates them to
    /// the TS Compiler API's UTF-16 character positions.
    ///
    /// Returns `None` when the host can't resolve a meaningful type:
    /// the file isn't in the project, no node sits at that span, the
    /// node has no type, or the worker failed. Callers treat `None` as
    /// "can't conclude anything" and emit no finding.
    fn type_at(&self, file: &Path, start_byte: u32, end_byte: u32) -> Option<TypeFacts>;
}
