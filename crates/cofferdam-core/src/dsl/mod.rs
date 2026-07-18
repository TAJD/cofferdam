//! v1 predicate DSL for `[invariants.scripted]` rules.
//!
//! This module implements the grammar specified in `docs/dsl-grammar.md`.
//!
//! # Module layout
//!
//! - [`ast`] — Rust types matching the EBNF: `TopPredicate`, `Predicate`,
//!   `Comparison`, `Subject`, `Op`, `Operand`, `Call`.
//! - [`parser`] — recursive-descent Pratt parser; entry points are
//!   [`parser::parse_top`] and [`parser::parse_predicate`].
//! - [`evaluator`] — evaluates parsed predicates against the graph runtime;
//!   entry points are [`evaluator::eval_top`] and [`evaluator::eval_predicate`].
//!   Wired into the engine via `Design.ScriptedInvariant`
//!   (`cofferdam-checks/src/design/scripted_invariant.rs`).
//!
//! # What is NOT here yet
//!
//! - Quantifiers, aggregation, cross-rule references (reserved for v2).

pub mod ast;
pub mod evaluator;
pub mod parser;

#[cfg(test)]
mod tests;
