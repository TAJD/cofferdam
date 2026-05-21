//! v1 predicate DSL for `[invariants.scripted]` rules.
//!
//! This module implements the grammar specified in `docs/dsl-grammar.md`.
//! It is intentionally scoped to parsing only (checkpoint 2 of 5).
//!
//! # Module layout
//!
//! - [`ast`] — Rust types matching the EBNF: `TopPredicate`, `Predicate`,
//!   `Comparison`, `Subject`, `Op`, `Operand`, `Call`.
//! - [`parser`] — recursive-descent Pratt parser; entry points are
//!   [`parser::parse_top`] and [`parser::parse_predicate`].
//!
//! # What is NOT here yet
//!
//! - Evaluator (checkpoint 3, `evaluator.rs` to be added).
//! - Engine wiring / `Design.ScriptedInvariant` (checkpoint 4).
//! - Quantifiers, aggregation, cross-rule references (reserved for v2).

pub mod ast;
pub mod parser;

#[cfg(test)]
mod tests;
