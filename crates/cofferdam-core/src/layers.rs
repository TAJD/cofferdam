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

/// One compiled layer — name, include/exclude sets, and a cached
/// specificity score used for cross-layer disambiguation.
pub struct LayerMatcher {
    pub name: String,
    /// Patterns without a leading `!` — a file must match at least one.
    include_set: globset::GlobSet,
    /// Patterns with a leading `!` (stripped) — a file must match none.
    exclude_set: globset::GlobSet,
    /// Longest non-glob prefix length across all include patterns.
    /// Used to pick the most-specific layer when multiple layers match.
    max_prefix_len: usize,
}

impl LayerMatcher {
    /// Returns `true` when the (already-normalized) path string belongs
    /// to this layer: at least one include matches and no exclude matches.
    fn matches(&self, normalized: &str) -> bool {
        self.include_set.is_match(normalized) && !self.exclude_set.is_match(normalized)
    }
}

/// Length of the longest non-glob prefix in a glob pattern string.
///
/// The non-glob prefix is the leading segment before the first glob
/// meta-character (`*`, `?`, `[`, `{`). Examples:
/// - `"components/ui/**"` → `"components/ui/"` → length 14
/// - `"components/**"`    → `"components/"`    → length 11
/// - `"**/*.test.ts"`     → `""`               → length 0
fn prefix_len(pattern: &str) -> usize {
    pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len())
}

/// Compile every layer's globs into a `LayerMatcher`. Bad globs are
/// silently dropped — a typo shouldn't blow up an analysis run; a future
/// config-validation pass should surface them at config-load time
/// instead.
pub fn build_matchers(cfg: &LayersConfig) -> Vec<LayerMatcher> {
    let mut out = Vec::with_capacity(cfg.layers.len());
    for (name, globs) in &cfg.layers {
        let mut include_builder = globset::GlobSetBuilder::new();
        let mut exclude_builder = globset::GlobSetBuilder::new();
        let mut max_prefix = 0usize;

        for g in globs {
            if let Some(exclude_pat) = g.strip_prefix('!') {
                if let Ok(glob) = globset::Glob::new(exclude_pat) {
                    exclude_builder.add(glob);
                }
            } else {
                if let Ok(glob) = globset::Glob::new(g) {
                    include_builder.add(glob);
                }
                // Track specificity regardless of whether the glob compiled
                // (if it didn't compile it can't match, so no effect on max).
                let pl = prefix_len(g);
                if pl > max_prefix {
                    max_prefix = pl;
                }
            }
        }

        let Ok(include_set) = include_builder.build() else {
            continue;
        };
        let Ok(exclude_set) = exclude_builder.build() else {
            continue;
        };

        out.push(LayerMatcher {
            name: name.clone(),
            include_set,
            exclude_set,
            max_prefix_len: max_prefix,
        });
    }
    out
}

/// Resolve `path` (absolute or project-relative) to a layer name, if any.
///
/// When multiple layers match, the one with the longest non-glob prefix
/// (highest specificity) wins. True ties break alphabetically by layer name,
/// which preserves the previous behavior for configs where layers don't
/// overlap.
pub fn layer_for(matchers: &[LayerMatcher], project_root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(project_root).unwrap_or(path);
    let normalized = rel.to_string_lossy().replace('\\', "/");

    matchers
        .iter()
        .filter(|m| m.matches(&normalized))
        .max_by(|a, b| {
            a.max_prefix_len
                .cmp(&b.max_prefix_len)
                // Tie-break alphabetically: *smaller* name sorts first, so
                // we reverse to pick it (`max_by` picks the "largest").
                .then_with(|| b.name.cmp(&a.name))
        })
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

    fn resolve(layers: &[(&str, &[&str])], path: &str) -> Option<String> {
        let c = cfg(layers);
        let matchers = build_matchers(&c);
        layer_for(&matchers, &c.project_root, Path::new(path))
    }

    // ── pre-existing tests (regression) ─────────────────────────────────

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

    // ── cd-31r: bead repro ───────────────────────────────────────────────

    /// Exact config from the bead: ui layer should win over components even
    /// though alphabetically "components" < "ui".
    #[test]
    fn bead_repro_ui_wins_components() {
        assert_eq!(
            resolve(
                &[
                    ("ui", &["components/ui/**"]),
                    ("components", &["components/**", "!components/ui/**"]),
                ],
                "/repo/components/ui/button.tsx"
            ),
            Some("ui".to_string())
        );
    }

    // ── cd-31r: regression guard ─────────────────────────────────────────

    /// Two non-overlapping layers: each file lands in its own layer.
    #[test]
    fn two_layers_no_overlap_each_owns_its_files() {
        let layers = &[
            ("app", &["src/app/**"][..]),
            ("infra", &["src/infra/**"][..]),
        ];
        assert_eq!(
            resolve(layers, "/repo/src/app/page.ts"),
            Some("app".to_string())
        );
        assert_eq!(
            resolve(layers, "/repo/src/infra/db.ts"),
            Some("infra".to_string())
        );
        assert_eq!(resolve(layers, "/repo/src/shared/util.ts"), None);
    }

    // ── cd-31r: three-way overlap ────────────────────────────────────────

    /// Three layers each claiming the same file with decreasing specificity.
    /// The most-specific one (`ui-button`, prefix `components/ui/button`)
    /// wins.
    #[test]
    fn three_way_overlap_most_specific_wins() {
        assert_eq!(
            resolve(
                &[
                    ("ui-button", &["components/ui/button.tsx"]),
                    ("ui", &["components/ui/**"]),
                    ("components", &["components/**"]),
                ],
                "/repo/components/ui/button.tsx"
            ),
            Some("ui-button".to_string())
        );
    }

    // ── cd-31r: true tie breaks alphabetically ───────────────────────────

    /// Two layers with the same prefix length (same specificity) both match.
    /// The alphabetically-first name should win.
    #[test]
    fn true_tie_breaks_alphabetically() {
        // Both "aaa" and "zzz" use "src/shared/**" — prefix "src/shared/"
        // (length 11). Alphabetical tie-break → "aaa" wins.
        assert_eq!(
            resolve(
                &[("aaa", &["src/shared/**"]), ("zzz", &["src/shared/**"]),],
                "/repo/src/shared/utils.ts"
            ),
            Some("aaa".to_string())
        );
    }

    // ── cd-31r: negation alone ───────────────────────────────────────────

    /// A single layer with an include and a negation: the negated subtree
    /// must not be in the layer.
    #[test]
    fn negation_alone_excludes_unmatched_file() {
        // lib claims src/** but NOT src/legacy/**
        assert_eq!(
            resolve(
                &[("lib", &["src/**", "!src/legacy/**"])],
                "/repo/src/legacy/foo.ts"
            ),
            None
        );
        // But a non-legacy file in src/ should still match.
        assert_eq!(
            resolve(
                &[("lib", &["src/**", "!src/legacy/**"])],
                "/repo/src/utils/helper.ts"
            ),
            Some("lib".to_string())
        );
    }

    // ── cd-31r: only-exclude pattern ────────────────────────────────────

    /// A layer with only exclude patterns (no include) cannot match any file.
    #[test]
    fn negation_with_no_matching_include_returns_none() {
        assert_eq!(
            resolve(&[("phantom", &["!foo/**"])], "/repo/foo/bar.ts"),
            None
        );
        assert_eq!(
            resolve(&[("phantom", &["!foo/**"])], "/repo/other/bar.ts"),
            None
        );
    }
}
