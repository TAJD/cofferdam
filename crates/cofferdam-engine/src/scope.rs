//! Per-check file-scope filtering (cd-81a.5).
//!
//! Each `CheckMeta` may declare a `FileScope` constraining which files
//! the check applies to. The engine compiles each scope into a fast
//! matcher once at construction time and uses it to skip inapplicable
//! files before paying the parse cost.
//!
//! Built-in checks rarely need a scope (they target all `.ts`/`.tsx`
//! the discovery layer already filters to). The mechanism exists for
//! plugins, where rovikore-host's `scannable?/1` boilerplate maps to a
//! declarative `extensions` + `path_pattern` + `exclude_patterns` block.
//!
//! Globs use the standard ignore-crate / gitignore syntax via
//! [`globset`]: `**/resources/**`, `**/dev_checks/**`, etc.

use std::path::Path;

use cofferdam_core::FileScope;
use globset::{Glob, GlobSet, GlobSetBuilder};

/// Compiled per-check file-scope matcher. `None` means "match every
/// file" — keeps the engine's hot path branchless for built-ins.
#[derive(Debug)]
pub struct ScopeMatcher {
    extensions: Vec<String>,
    include: Option<GlobSet>,
    excludes: GlobSet,
}

impl ScopeMatcher {
    pub fn from_meta(scope: Option<&FileScope>) -> Result<Option<Self>, ScopeError> {
        let Some(scope) = scope else {
            return Ok(None);
        };
        let extensions = scope.extensions.iter().map(|s| s.to_string()).collect();

        let include = match scope.path_pattern {
            Some(p) => Some(compile_glob(&[p])?),
            None => None,
        };
        let excludes = compile_glob(scope.exclude_patterns)?;

        Ok(Some(Self {
            extensions,
            include,
            excludes,
        }))
    }

    /// True when the file at `path` should be analysed by the check
    /// owning this scope. Cheap (no allocation; globset is bytewise).
    pub fn matches(&self, path: &Path) -> bool {
        // Extension filter — empty list means "any".
        if !self.extensions.is_empty() {
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                return false;
            };
            if !self.extensions.iter().any(|e| e == ext) {
                return false;
            }
        }
        // Include glob — when present, the path must match it.
        if let Some(set) = &self.include {
            if !set.is_match(path) {
                return false;
            }
        }
        // Exclude globs — any match disqualifies.
        if self.excludes.is_match(path) {
            return false;
        }
        true
    }
}

fn compile_glob(patterns: &[&str]) -> Result<GlobSet, ScopeError> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let glob = Glob::new(p).map_err(|source| ScopeError::Compile {
            pattern: p.to_string(),
            source,
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| ScopeError::Compile {
        pattern: "<set>".to_string(),
        source,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error("invalid glob `{pattern}`: {source}")]
    Compile {
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_scope_matches_everything() {
        let m = ScopeMatcher::from_meta(None).unwrap();
        assert!(m.is_none());
    }

    #[test]
    fn extension_filter_keeps_only_listed() {
        let scope = FileScope {
            extensions: &["tsx"],
            path_pattern: None,
            exclude_patterns: &[],
        };
        let m = ScopeMatcher::from_meta(Some(&scope)).unwrap().unwrap();
        assert!(m.matches(Path::new("src/comp.tsx")));
        assert!(!m.matches(Path::new("src/comp.ts")));
        assert!(!m.matches(Path::new("README.md")));
    }

    #[test]
    fn path_pattern_constrains_subtree() {
        let scope = FileScope {
            extensions: &["ts"],
            path_pattern: Some("**/resources/**"),
            exclude_patterns: &[],
        };
        let m = ScopeMatcher::from_meta(Some(&scope)).unwrap().unwrap();
        assert!(m.matches(Path::new("src/resources/orders.ts")));
        assert!(!m.matches(Path::new("src/handlers/orders.ts")));
    }

    #[test]
    fn exclude_overrides_include() {
        let scope = FileScope {
            extensions: &["ts"],
            path_pattern: Some("src/**"),
            exclude_patterns: &["**/dev_checks/**"],
        };
        let m = ScopeMatcher::from_meta(Some(&scope)).unwrap().unwrap();
        assert!(m.matches(Path::new("src/handlers/x.ts")));
        assert!(!m.matches(Path::new("src/dev_checks/y.ts")));
    }

    #[test]
    fn invalid_glob_returns_compile_error() {
        let scope = FileScope {
            extensions: &[],
            path_pattern: Some("[unclosed"),
            exclude_patterns: &[],
        };
        let err = ScopeMatcher::from_meta(Some(&scope)).unwrap_err();
        assert!(matches!(err, ScopeError::Compile { .. }));
    }
}
