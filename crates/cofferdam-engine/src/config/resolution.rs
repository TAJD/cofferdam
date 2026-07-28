//! Configuration resolution with invariants merging.

use std::path::{Component, Path, PathBuf};

use cofferdam_core::graph::LayersConfig;
use cofferdam_core::invariants;

use super::{ConfigError, ProjectConfig};
use crate::config::loader;

/// Errors that can prevent `cofferdam.toml` from loading. Includes IO
/// failure, malformed TOML, and schema-validation failures (each
/// variant carries the source path so the CLI can format actionable
/// diagnostics).
#[derive(Debug, Default)]
pub struct LoadDiagnostics {
    /// Warning to emit (or empty). Not an error — the load still
    /// succeeded with whatever was loadable.
    pub warnings: Vec<String>,
}

/// End-to-end config resolution. Looks for `cofferdam.toml` (walking up
/// from `cwd` unless `explicit_toml` is given), and `cofferdam.invariants.toml`
/// alongside or up the same chain. Returns:
/// * `(Some(cfg), Some(toml_path))` when cofferdam.toml was found.
/// * `(Some(cfg), None)` when only invariants.toml was found — `cfg` has
///   default per-check options + populated invariants spec.
/// * `(None, None)` when neither file is present.
///
/// Discovery is non-fatal — a missing-or-broken invariants.toml only
/// emits a warning via the returned `LoadDiagnostics` (CLI surface).
#[allow(clippy::result_large_err)] // ConfigError carries diagnostic context; rare-path code, size irrelevant
pub fn resolve_with_invariants(
    explicit_toml: Option<&Path>,
    cwd: &Path,
    no_config: bool,
) -> Result<
    (
        Option<ProjectConfig>,
        Option<std::path::PathBuf>,
        LoadDiagnostics,
    ),
    ConfigError,
> {
    let mut diags = LoadDiagnostics::default();
    if no_config {
        return Ok((None, None, diags));
    }

    let toml_path = match explicit_toml {
        Some(p) => Some(p.to_path_buf()),
        None => loader::discover(cwd),
    };

    let (mut cfg, returned_path) = match toml_path {
        Some(p) => match loader::load(&p) {
            Ok(c) => (c, Some(p)),
            Err(e) => {
                if explicit_toml.is_some() {
                    return Err(e);
                }
                diags.warnings.push(format!("ignoring config ({e})"));
                (ProjectConfig::default(), None)
            }
        },
        None => (ProjectConfig::default(), None),
    };

    // Discovery base for invariants.toml: cofferdam.toml's parent if we
    // loaded one, otherwise cwd. Mirrors how cofferdam.toml's own
    // discovery walks from cwd in the no-config case.
    let invariants_start = returned_path
        .as_ref()
        .and_then(|p| p.parent())
        .unwrap_or(cwd);
    match merge_invariants_from(&mut cfg, invariants_start) {
        Err(ConfigError::Invariants(e)) if e.is_fatal() => {
            // Schema-version errors and other fatal invariants errors
            // mean "the spec exists but cannot safely be loaded". Fail
            // loudly rather than silently ignoring the user's
            // architectural rules.
            return Err(ConfigError::Invariants(e));
        }
        Err(e) => {
            diags
                .warnings
                .push(format!("ignoring cofferdam.invariants.toml ({e})"));
        }
        Ok(_) => {
            if cfg.layers_double_declaration {
                diags.warnings.push(
                    "[layers] declared in both cofferdam.toml and cofferdam.invariants.toml — invariants.toml takes precedence; remove [layers] from cofferdam.toml to silence this hint".to_string()
                );
            }
            if let Some(spec) = cfg.invariants.as_ref() {
                if spec.schema_version_deprecated {
                    diags.warnings.push(format!(
                        "cofferdam.invariants.toml declares schema_version {} which is older than the current {}; update the spec to silence this hint (see docs/schema-versioning.md)",
                        spec.schema_version,
                        cofferdam_core::invariants::CURRENT_SCHEMA_VERSION,
                    ));
                } else if !spec.schema_version_explicit
                    && (!spec.layers.is_empty()
                        || !spec.boundaries.is_empty()
                        || !spec.invariants.is_empty()
                        || !spec.public_api.exports.is_empty()
                        || !spec.scripted.is_empty())
                {
                    diags.warnings.push(format!(
                        "cofferdam.invariants.toml is missing `schema_version`; assumed {}. Add `schema_version = \"{}\"` at the top of the file to silence this hint (see docs/schema-versioning.md)",
                        cofferdam_core::invariants::CURRENT_SCHEMA_VERSION,
                        cofferdam_core::invariants::CURRENT_SCHEMA_VERSION,
                    ));
                }
            }
        }
    }

    // Decide whether we have anything worth returning. cofferdam.toml's
    // presence (returned_path) OR a populated invariants spec both
    // count.
    let have_invariants = cfg
        .invariants
        .as_ref()
        .map(|spec| {
            !spec.layers.is_empty()
                || !spec.boundaries.is_empty()
                || !spec.invariants.is_empty()
                || !spec.public_api.exports.is_empty()
                || !spec.scripted.is_empty()
        })
        .unwrap_or(false);

    if returned_path.is_none() && !have_invariants {
        return Ok((None, None, diags));
    }

    Ok((Some(cfg), returned_path, diags))
}

/// Config resolution anchored on the paths the user asked to analyze
/// (CD-149).
///
/// `cofferdam check src/app` run from a monorepo root must pick up
/// `src/app/cofferdam.toml`; walking up from the process CWD never
/// reaches it. This resolves against the common ancestor of `roots`
/// first and falls back to `cwd` when that anchor turns up nothing, so
/// the no-argument / `cofferdam check .` case is byte-for-byte
/// unchanged.
///
/// An explicit `--config` path (or `--no-config`) short-circuits both
/// anchors, as before.
#[allow(clippy::result_large_err)]
pub fn resolve_for_targets(
    explicit: Option<&Path>,
    roots: &[PathBuf],
    cwd: &Path,
    no_config: bool,
) -> Result<
    (
        Option<ProjectConfig>,
        Option<std::path::PathBuf>,
        LoadDiagnostics,
    ),
    ConfigError,
> {
    if no_config || explicit.is_some() {
        return resolve_with_invariants(explicit, cwd, no_config);
    }
    match target_anchor(roots, cwd) {
        Some(anchor)
            if anchor != normalize(cwd)
                && (loader::discover(&anchor).is_some()
                    || invariants::discover(&anchor).is_some()) =>
        {
            resolve_with_invariants(None, &anchor, false)
        }
        _ => resolve_with_invariants(None, cwd, false),
    }
}

/// Directory to start walk-up discovery from, given the analyzed roots.
/// Files contribute their parent directory; the result is the deepest
/// directory that is an ancestor of every root. `None` when there are no
/// roots or they share no common prefix (disjoint Windows drives) — the
/// caller then falls back to the CWD anchor.
fn target_anchor(roots: &[PathBuf], cwd: &Path) -> Option<PathBuf> {
    let mut acc: Option<PathBuf> = None;
    for root in roots {
        let abs = normalize(&cwd.join(root));
        let dir = if abs.is_file() {
            abs.parent()?.to_path_buf()
        } else {
            abs
        };
        acc = Some(match acc {
            None => dir,
            Some(prev) => common_ancestor(&prev, &dir)?,
        });
    }
    acc
}

/// Lexical path normalization: drops `.` segments and resolves `..`
/// without touching the filesystem. `std::path::absolute` leaves `..` in
/// place on Unix, which would make two spellings of the same directory
/// compare unequal.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Longest shared component prefix of two normalized absolute paths.
fn common_ancestor(a: &Path, b: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    let mut any = false;
    for (ca, cb) in a.components().zip(b.components()) {
        if ca != cb {
            break;
        }
        out.push(ca.as_os_str());
        any = true;
    }
    any.then_some(out)
}

/// Discover and load `cofferdam.invariants.toml` from the same starting
/// directory used to find `cofferdam.toml`, then merge into `cfg`.
///
/// Merge rules:
/// * If invariants.toml has any `[layers]` data, it replaces
///   `cfg.layers` wholesale. Sets `layers_double_declaration` when
///   cofferdam.toml also had a `[layers]` block, so the CLI can warn.
/// * The full spec is stashed on `cfg.invariants` for downstream
///   consumers (engine corpus publish, OrphanExport, BoundaryFrozen,
///   InvariantViolation).
///
/// `start` is typically the directory of `cofferdam.toml` (or the cwd
/// when no cofferdam.toml exists). Loaders that have already located
/// `cofferdam.toml` should pass its parent here.
#[allow(clippy::result_large_err)]
pub fn merge_invariants_from(
    cfg: &mut ProjectConfig,
    start: &Path,
) -> Result<Option<std::path::PathBuf>, ConfigError> {
    let Some(path) = invariants::discover(start) else {
        return Ok(None);
    };
    let spec = invariants::load(&path)?;

    // Layers merge: invariants.toml wins. Empty layers block in
    // invariants.toml leaves cfg.layers untouched.
    if !spec.layers.is_empty() {
        let had_existing = cfg.layers.is_some();
        cfg.layers = Some(LayersConfig {
            project_root: spec.project_root.clone(),
            layers: spec.layers.clone(),
            allow: spec.layers_allow.clone(),
        });
        cfg.layers_double_declaration = had_existing;
    }

    cfg.invariants = Some(spec);
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a `cofferdam.toml` whose `[budgets]` marker identifies it.
    fn write_config(dir: &Path, marker: u32) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("cofferdam.toml"),
            format!("[budgets]\n\"Warning.NoConsoleLog\" = {marker}\n"),
        )
        .unwrap();
    }

    /// Bound walk-up discovery at `dir`, so tests never inherit a
    /// `cofferdam.toml` from a real ancestor of the temp directory.
    fn seal(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    fn marker_of(cfg: &Option<ProjectConfig>) -> Option<u32> {
        cfg.as_ref()?.budgets.get("Warning.NoConsoleLog").copied()
    }

    fn resolve(roots: &[&str], cwd: &Path) -> Option<u32> {
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        let (cfg, _, _) = resolve_for_targets(None, &roots, cwd, false).unwrap();
        marker_of(&cfg)
    }

    /// root/ holds config 1, root/sub/ holds config 2.
    fn fixture() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        seal(root);
        write_config(root, 1);
        write_config(&root.join("sub"), 2);
        std::fs::create_dir_all(root.join("other")).unwrap();
        td
    }

    #[test]
    fn target_dir_config_wins_over_cwd() {
        let td = fixture();
        assert_eq!(resolve(&["sub"], td.path()), Some(2));
    }

    #[test]
    fn absolute_target_dir_config_wins_over_cwd() {
        let td = fixture();
        let sub = td.path().join("sub");
        assert_eq!(resolve(&[sub.to_str().unwrap()], td.path()), Some(2));
    }

    #[test]
    fn file_target_anchors_on_its_parent() {
        let td = fixture();
        std::fs::write(td.path().join("sub/a.ts"), "export const a = 1;\n").unwrap();
        assert_eq!(resolve(&["sub/a.ts"], td.path()), Some(2));
    }

    #[test]
    fn dot_root_keeps_cwd_behaviour() {
        let td = fixture();
        assert_eq!(resolve(&["."], td.path()), Some(1));
    }

    #[test]
    fn no_roots_keeps_cwd_behaviour() {
        let td = fixture();
        assert_eq!(resolve(&[], td.path()), Some(1));
    }

    #[test]
    fn disjoint_roots_anchor_on_common_ancestor() {
        let td = fixture();
        assert_eq!(resolve(&["sub", "other"], td.path()), Some(1));
    }

    #[test]
    fn target_without_config_falls_back_to_cwd() {
        let cwd = tempfile::tempdir().unwrap();
        seal(cwd.path());
        write_config(cwd.path(), 1);

        let target = tempfile::tempdir().unwrap();
        seal(target.path());

        assert_eq!(
            resolve(&[target.path().to_str().unwrap()], cwd.path()),
            Some(1)
        );
    }

    #[test]
    fn explicit_config_overrides_target_anchor() {
        let td = fixture();
        let explicit = td.path().join("cofferdam.toml");
        let roots = vec![PathBuf::from("sub")];
        let (cfg, path, _) =
            resolve_for_targets(Some(&explicit), &roots, td.path(), false).unwrap();
        assert_eq!(marker_of(&cfg), Some(1));
        assert_eq!(path, Some(explicit));
    }

    #[test]
    fn no_config_short_circuits() {
        let td = fixture();
        let roots = vec![PathBuf::from("sub")];
        let (cfg, path, _) = resolve_for_targets(None, &roots, td.path(), true).unwrap();
        assert!(cfg.is_none() && path.is_none());
    }

    #[test]
    fn normalize_resolves_dot_and_parent_segments() {
        assert_eq!(
            normalize(Path::new("/a/./b/../c")),
            PathBuf::from("/a").join("c")
        );
    }

    #[test]
    fn common_ancestor_of_unrelated_roots_is_none() {
        assert!(common_ancestor(Path::new("a/b"), Path::new("x/y")).is_none());
    }
}
