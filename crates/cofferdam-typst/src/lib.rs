//! `cofferdam-typst` — a standalone linter for Typst package directories
//! bound for [Typst Universe](https://typst.app/universe/).
//!
//! Not wired into `cofferdam-engine`: the unit of analysis is a PACKAGE
//! DIRECTORY (`typst.toml` + `LICENSE` + `README.md` + bundle), which
//! doesn't fit the engine's per-file AST-oriented `Check` trait. This
//! crate defines its own lightweight [`check::TypstCheck`] trait and
//! reuses `cofferdam_core::Issue` for output, so findings render through
//! the same formatters as every other cofferdam finding.
//!
//! Entry point: `cofferdam typst <dir>` (wired in `cofferdam-cli`).

pub mod check;
pub mod checks;
pub mod manifest;
pub mod package;

pub use check::{all_typst_checks, TypstCheck, TypstCheckMeta};
pub use manifest::{Manifest, ManifestSpans};
pub use package::{load, LoadError, TypstPackage};
