//! `Adapter` — descriptive registration trait for language pipelines.
//!
//! This is a DOCUMENTED CONVENTION for this iteration, not something the type
//! system or engine enforces. An `Adapter` impl declares, for one `Language`,
//! which files it claims and which schema namespace its corpus/graph facts
//! live under. The intended one-way contract is: adapters produce facts from
//! source; they do not see rule configuration and do not construct `Issue`s
//! directly (that stays the `Check` trait's job). Nothing here stops an
//! adapter impl from violating that contract today — there is no runtime
//! enforcement, no sandboxing, and no wiring into `cofferdam-engine`'s
//! discovery/parse/dispatch loop. See `docs/adapter-contract.md` for the
//! full picture, including what's deferred.
use crate::source::Language;

/// Describes a language pipeline's identity: which `Language` it produces
/// facts for, which files it claims, and which schema namespace its facts
/// live under. See the module doc for the (unenforced) one-way contract.
pub trait Adapter {
    /// The `Language` variant this adapter produces facts for.
    fn language(&self) -> Language;

    /// Glob patterns / extensions this adapter claims, mirroring
    /// `Language::from_path`'s extension list for the same language.
    fn file_globs(&self) -> &[&str];

    /// The schema-extension namespace this adapter's corpus/graph facts
    /// live under (e.g. `"ts"`, `"rust"`).
    fn schema_namespace(&self) -> &str;
}

/// Adapter impl describing the existing TypeScript/JS pipeline (oxc-based).
pub struct TypeScriptAdapter;

impl Adapter for TypeScriptAdapter {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn file_globs(&self) -> &[&str] {
        &[
            "*.ts", "*.tsx", "*.js", "*.jsx", "*.cjs", "*.mjs", "*.mts", "*.cts",
        ]
    }

    fn schema_namespace(&self) -> &str {
        "ts"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_adapter_language() {
        assert_eq!(TypeScriptAdapter.language(), Language::TypeScript);
    }

    #[test]
    fn typescript_adapter_file_globs() {
        assert_eq!(
            TypeScriptAdapter.file_globs(),
            &["*.ts", "*.tsx", "*.js", "*.jsx", "*.cjs", "*.mjs", "*.mts", "*.cts"]
        );
    }

    #[test]
    fn typescript_adapter_schema_namespace() {
        assert_eq!(TypeScriptAdapter.schema_namespace(), "ts");
    }
}
