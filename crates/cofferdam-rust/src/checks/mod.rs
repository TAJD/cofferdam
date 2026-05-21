//! Per-check modules for the Rust adapter (cd-91zc).
//!
//! Each check ships in its own file and is registered via
//! [`all_rust_checks`]. The set is currently empty in
//! `cofferdam-checks::all_builtins()` — the engine wiring lands in
//! cd-91zc checkpoint 4 (the `Language` enum on `SourceFile` + per-file
//! dispatch routing). Until then the checks are callable from tests
//! and from anything that explicitly constructs them.

pub mod missing_pub_doc;
pub mod no_unimplemented_in_non_test;
pub mod no_unwrap_in_lib;

use cofferdam_core::Check;

/// Every Rust adapter check that should run on a `Language::Rust`
/// source file. Engine wiring in checkpoint 4 calls this from
/// `cofferdam-engine`'s per-language dispatch.
pub fn all_rust_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(no_unwrap_in_lib::NoUnwrapInLib),
        Box::new(no_unimplemented_in_non_test::NoUnimplementedInNonTest),
        Box::new(missing_pub_doc::MissingPubDoc),
    ]
}
