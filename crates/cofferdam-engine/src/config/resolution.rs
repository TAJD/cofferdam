//! Configuration resolution with invariants merging.

use std::path::Path;

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
