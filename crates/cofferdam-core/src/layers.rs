//! Layer-membership helpers.
//!
//! Compiles a `LayersConfig` into glob matchers and answers "what layer is
//! this file in?". Shared between `Design.LayerViolation` (which evaluates
//! cross-layer import edges) and `cofferdam advise` (which reports per-file
//! layer membership in advisory output). Lives next to `LayersConfig` in
//! core because both consumers want the answer without depending on the
//! engine.

use std::path::Path;

use crate::graph::LayersConfig;

/// One compiled layer — name plus its glob-set.
pub struct LayerMatcher {
    pub name: String,
    pub set: globset::GlobSet,
}

/// Compile every layer's globs into a `LayerMatcher`. Bad globs are
/// silently dropped — a typo shouldn't blow up an analysis run; a future
/// config-validation pass should surface them at config-load time
/// instead.
pub fn build_matchers(cfg: &LayersConfig) -> Vec<LayerMatcher> {
    let mut out = Vec::with_capacity(cfg.layers.len());
    for (name, globs) in &cfg.layers {
        let mut builder = globset::GlobSetBuilder::new();
        for g in globs {
            if let Ok(glob) = globset::Glob::new(g) {
                builder.add(glob);
            }
        }
        if let Ok(set) = builder.build() {
            out.push(LayerMatcher {
                name: name.clone(),
                set,
            });
        }
    }
    out
}

/// Resolve `path` (absolute or project-relative) to a layer name, if any.
/// First-match wins — authors place more specific entries earlier (today
/// `BTreeMap` orders alphabetically; advisory ordering follows suit).
pub fn layer_for(matchers: &[LayerMatcher], project_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project_root).unwrap_or(path);
    let normalized = rel.to_string_lossy().replace('\\', "/");
    matchers
        .iter()
        .find(|m| m.set.is_match(&normalized))
        .map(|m| m.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn cfg(layers: &[(&str, &[&str])]) -> LayersConfig {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, globs) in layers {
            map.insert(
                (*name).to_string(),
                globs.iter().map(|s| (*s).to_string()).collect(),
            );
        }
        LayersConfig {
            project_root: PathBuf::from("/repo"),
            layers: map,
            allow: BTreeMap::new(),
        }
    }

    #[test]
    fn matches_first_layer_whose_glob_matches() {
        let c = cfg(&[("app", &["src/app/**"]), ("domain", &["src/domain/**"])]);
        let matchers = build_matchers(&c);
        assert_eq!(
            layer_for(
                &matchers,
                &c.project_root,
                Path::new("/repo/src/app/page.ts")
            ),
            Some("app".to_string())
        );
        assert_eq!(
            layer_for(
                &matchers,
                &c.project_root,
                Path::new("/repo/src/domain/user.ts")
            ),
            Some("domain".to_string())
        );
    }

    #[test]
    fn returns_none_when_no_layer_matches() {
        let c = cfg(&[("app", &["src/app/**"])]);
        let matchers = build_matchers(&c);
        assert_eq!(
            layer_for(&matchers, &c.project_root, Path::new("/repo/scripts/x.ts")),
            None
        );
    }

    #[test]
    fn handles_absolute_paths_under_project_root() {
        let c = cfg(&[("app", &["src/app/**"])]);
        let matchers = build_matchers(&c);
        let p: PathBuf = ["src", "app", "page.ts"].iter().collect();
        let abs = c.project_root.join(p);
        assert_eq!(
            layer_for(&matchers, &c.project_root, &abs),
            Some("app".to_string())
        );
    }

    #[test]
    fn bad_globs_are_silently_dropped() {
        let c = cfg(&[("good", &["src/good/**"]), ("bad", &["[unterminated"])]);
        let matchers = build_matchers(&c);
        assert_eq!(
            layer_for(&matchers, &c.project_root, Path::new("/repo/src/good/x.ts")),
            Some("good".to_string())
        );
    }
}
