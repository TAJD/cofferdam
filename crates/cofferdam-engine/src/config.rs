//! Project config loader — `cofferdam.toml`.
//!
//! Per-check option overrides for the engine, sourced from a TOML file
//! at the project root. The schema is deliberately small in v0:
//!
//! ```toml
//! [checks."Readability.MaxLineLength"]
//! limit = 120
//! severity = "warning"   # phase-3 (cd-t1a) — accepted, not yet enforced
//! enabled = true         # phase-3 — accepted, not yet enforced
//!
//! [checks."Readability.MaxFunctionLength"]
//! limit = 50
//!
//! [checks."Design.MaxParameters"]
//! limit = 5
//! ```
//!
//! ## Discovery
//!
//! `discover()` walks up from the starting directory until either
//! `cofferdam.toml` is found or a `.git` directory is reached (i.e. the
//! repo root). Stopping at `.git` keeps a stray `cofferdam.toml` in a
//! parent directory from accidentally configuring an unrelated repo.
//!
//! ## Precedence
//!
//! Values cascade through CLI flag > env var > config file > schema
//! default. Today only file > default is wired; the CLI and env layers
//! plug in above this loader without changing it.
//!
//! ## Why `toml` and not `figment`
//!
//! cofferdam-core deliberately stays dep-light. `toml` alone is enough
//! for v0 (single file, no env/cli layering at this seam yet). Upgrade
//! to figment if/when env-var support lands and the layering matters at
//! this layer.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use cofferdam_core::graph::LayersConfig;
use cofferdam_core::invariants::{self, InvariantsError, InvariantsSpec};
use cofferdam_core::{validate_options, CheckOptions, OptionsError, RawOptionValue, Severity};
use serde::Deserialize;

/// Filename loaders look for during walk-up discovery.
pub const FILE_NAME: &str = "cofferdam.toml";

/// Meta-keys that may appear inside `[checks."X.Y"]` blocks but aren't
/// per-check options. `severity` (cd-t1a) is now first-class — extracted
/// into `ProjectConfig::severity_overrides` rather than passed through
/// to `validate_options`. `enabled` is still a forward-compatible
/// placeholder (no behaviour wired yet).
const META_KEYS: &[&str] = &["severity", "enabled"];

/// Top-level `cofferdam.toml` keys that users sometimes accidentally nest
/// under a `[checks."X.Y"]` table because TOML's lexical scoping assigns
/// all keys after a table header to that table. When one of these appears
/// as an unknown option, we emit a directive error instead of the generic
/// "check does not declare option '...'" message.
const MISPLACED_TOP_LEVEL_KEYS: &[&str] = &["plugins", "extends", "include"];

/// Parsed project config: per-check raw option bags + per-check
/// severity overrides. Unknown check IDs are stored verbatim and
/// surfaced via `unknown_check_ids` so the CLI can warn without
/// failing the build over a typo.
#[derive(Debug, Clone, Default)]
pub struct ProjectConfig {
    pub checks: BTreeMap<String, BTreeMap<String, RawOptionValue>>,
    /// Per-check severity overrides parsed from `[checks."X.Y"] severity = "..."`.
    /// Keyed by check_id. Engine consults this in its severity post-pass.
    pub severity_overrides: BTreeMap<String, Severity>,
    /// `[layers]` block. `None` when the table is missing — keeps the
    /// `Design.LayerViolation` check a no-op for projects that haven't
    /// declared an architecture. The `project_root` field is filled in
    /// after parsing (load knows the path; parse doesn't).
    pub layers: Option<LayersConfig>,
    /// `plugins = [...]` array — paths (or package specifiers) that
    /// resolve to local Node.js plugin modules implementing the
    /// `@cofferdam/check-sdk` `defineCheck` shape. Resolved relative to
    /// the config file's directory. Empty when no plugins declared.
    pub plugins: Vec<PathBuf>,
    /// Parsed `cofferdam.invariants.toml` spec, when one was discovered
    /// alongside the cofferdam.toml. Layers from this spec take
    /// precedence over the cofferdam.toml `[layers]` block; the engine
    /// merges before publishing into the LAYERS corpus slot.
    pub invariants: Option<InvariantsSpec>,
    /// Set when both `cofferdam.toml` `[layers]` AND
    /// `cofferdam.invariants.toml` `[layers]` are populated. Surfaced by
    /// the CLI as a one-line deprecation hint.
    pub layers_double_declaration: bool,
    /// `[engine] type_aware` — `Some(false)` force-disables type-aware
    /// checks (those declaring `CheckMeta::requires_types`) even when one
    /// is registered, so CI machines without a Node runtime don't pay the
    /// type-host cost or see "type host unavailable" warnings (cd-9hp.2.4).
    /// `None` (the default) means enabled; the engine still auto-opts-out
    /// when no `requires_types` check is registered. Read via
    /// [`ProjectConfig::type_aware_enabled`].
    pub engine_type_aware: Option<bool>,
    /// `[[overrides]]` blocks — per-path-glob check config (cd-m5tu).
    /// Each block scopes `limit`/`severity`/`disabled` (and any other
    /// per-check option) to files matching its `paths` globs, while
    /// every other check keeps running on those files. Empty when none
    /// are declared. The engine applies them per-file in declaration
    /// order; the last matching block wins per (check, key).
    pub overrides: Vec<OverrideBlock>,
}

/// One `[[overrides]]` block: a set of path globs plus the per-check
/// config that applies to files matching any of them (cd-m5tu).
///
/// Globs are written project-root-relative in forward-slash form
/// (`**/*.test.tsx`, `src/legacy/**`) and matched the same way
/// `[public_api].exports` globs are — see [`OverrideBlock::is_match`].
#[derive(Debug, Clone)]
pub struct OverrideBlock {
    /// Raw glob patterns as written, kept for diagnostics and hashing.
    pub paths: Vec<String>,
    /// Compiled matcher over `paths`.
    pub globset: globset::GlobSet,
    /// Normalised absolute root prefix used to reduce an absolute file
    /// key to the project-relative form the globs are written against.
    pub root_key: String,
    /// Per-check overrides, keyed by check id.
    pub checks: BTreeMap<String, OverrideCheck>,
}

/// The config a single `[[overrides]]` block applies to one check on
/// matching files. Only the fields the block actually sets are
/// populated; the engine overlays them onto the global per-check
/// config (cd-m5tu).
#[derive(Debug, Clone, Default)]
pub struct OverrideCheck {
    /// Option keys this block sets (e.g. `limit`). Overlaid over the
    /// global per-check option bag for matching files.
    pub options: BTreeMap<String, RawOptionValue>,
    /// `severity = "..."` for this check on matching files. `None`
    /// leaves the global severity in place.
    pub severity: Option<Severity>,
    /// `disabled = true` skips the check entirely for matching files;
    /// `disabled = false` re-enables it (so a later block can undo an
    /// earlier one). `None` leaves the prior decision untouched.
    pub disabled: Option<bool>,
}

impl OverrideBlock {
    /// Test whether `file_key` (a forward-slash, normalised path — may
    /// be absolute or relative) is matched by this block's globs.
    /// Mirrors `public_api`'s matcher: strip the absolute root prefix
    /// (or a leading `./`) so root-relative patterns match an absolute
    /// engine-promoted path.
    pub fn is_match(&self, file_key: &str) -> bool {
        let rel = {
            let with_slash = if self.root_key.ends_with('/') {
                self.root_key.clone()
            } else {
                format!("{}/", self.root_key)
            };
            if let Some(stripped) = file_key.strip_prefix(with_slash.as_str()) {
                stripped
            } else if let Some(stripped) = file_key.strip_prefix(&self.root_key) {
                stripped.trim_start_matches('/')
            } else {
                file_key.trim_start_matches("./")
            }
        };
        self.globset.is_match(rel)
    }
}

impl ProjectConfig {
    /// Whether type-aware checks may run. `false` only when the user set
    /// `[engine] type_aware = false`; the default is enabled. The CLI
    /// consults this before spawning the ts-morph type-host worker.
    pub fn type_aware_enabled(&self) -> bool {
        self.engine_type_aware.unwrap_or(true)
    }
}

/// Errors that can prevent `cofferdam.toml` from loading. Includes IO
/// failure, malformed TOML, and schema-validation failures (each
/// variant carries the source path so the CLI can format actionable
/// diagnostics).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error(
        "config {path}: in [checks.\"{check_id}\"], option `{key}` has unsupported type ({reason})"
    )]
    UnsupportedValue {
        path: PathBuf,
        check_id: String,
        key: String,
        reason: &'static str,
    },
    #[error("config {path}: in [checks.\"{check_id}\"], severity must be a string but got {got}")]
    SeverityNotString {
        path: PathBuf,
        check_id: String,
        got: &'static str,
    },
    #[error("config {path}: in [checks.\"{check_id}\"], {source}")]
    BadSeverity {
        path: PathBuf,
        check_id: String,
        #[source]
        source: cofferdam_core::ParseSeverityError,
    },
    #[error("config {path}: option validation failed for [checks.\"{check_id}\"]: {source}")]
    Validate {
        path: PathBuf,
        check_id: String,
        #[source]
        source: OptionsError,
    },
    #[error(
        "config {path}: `{intended_key}` looks like a top-level cofferdam.toml key, \
but it appears nested under [checks.\"{nested_under}\"] because TOML treats keys after a \
table header as belonging to that table. \
Move `{intended_key} = ...` ABOVE the first [checks.\"...\"] table (or to the top of the \
file) and re-run."
    )]
    MisplacedTopLevelKey {
        path: PathBuf,
        intended_key: String,
        nested_under: String,
    },
    #[error(transparent)]
    Invariants(#[from] InvariantsError),
}

/// TOML document layout. Top-level sections grow additively here.
#[derive(Debug, Deserialize, Default)]
struct TomlDoc {
    #[serde(default)]
    checks: BTreeMap<String, toml::Value>,
    /// `[layers]` for `Design.LayerViolation`. Each top-level key is
    /// either a layer name (whose value is a string array of globs) or
    /// the reserved sub-table `allow` mapping layer names to lists of
    /// allowed dependency layers.
    #[serde(default)]
    layers: BTreeMap<String, toml::Value>,
    /// `plugins = [...]` — Node-side plugin module paths (cd-7e4 / cd-81a.7).
    /// Resolved relative to the config file's directory.
    #[serde(default)]
    plugins: Vec<String>,
    /// `[engine]` — engine-level toggles. Today just `type_aware`
    /// (cd-9hp.2.4); grows additively.
    #[serde(default)]
    engine: EngineSection,
    /// `[[overrides]]` — per-path-glob check config (cd-m5tu). Array of
    /// tables, each with `paths = [...]` and a `[overrides.checks."X"]`
    /// sub-table.
    #[serde(default)]
    overrides: Vec<OverrideTable>,
}

/// Raw `[[overrides]]` element. `checks` keys are check ids; each value
/// is a table of option/`severity`/`disabled` entries, parsed the same
/// way a `[checks."X"]` block is.
#[derive(Debug, Deserialize, Default)]
struct OverrideTable {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    checks: BTreeMap<String, toml::Value>,
}

/// `[engine]` table. Unknown keys are ignored (forward-compatible).
#[derive(Debug, Deserialize, Default)]
struct EngineSection {
    /// `type_aware = false` force-disables type-aware checks. Absent
    /// means enabled (the default).
    #[serde(default)]
    type_aware: Option<bool>,
}

/// Walk up from `start` looking for `cofferdam.toml`. Stops at the
/// directory containing a `.git` entry (the repo root) to avoid
/// accidentally inheriting config from an unrelated parent project.
/// Returns `None` when nothing is found before the repo or filesystem
/// root.
pub fn discover(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Stop the walk at the repo root.
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}

/// Parse a config file from disk. Loud failures: any IO error, any TOML
/// parse error, any per-option type the schema doesn't understand.
/// Quiet successes: unknown check IDs are stored verbatim and surfaced
/// at validation time (where we know which checks the engine has).
#[allow(clippy::result_large_err)] // ConfigError carries diagnostic context; rare-path code, size irrelevant
pub fn load(path: &Path) -> Result<ProjectConfig, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse(path, &raw)
}

#[allow(clippy::result_large_err)]
fn parse(path: &Path, raw: &str) -> Result<ProjectConfig, ConfigError> {
    let doc: TomlDoc = toml::from_str(raw).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let mut checks: BTreeMap<String, BTreeMap<String, RawOptionValue>> = BTreeMap::new();
    let mut severity_overrides: BTreeMap<String, Severity> = BTreeMap::new();
    for (check_id, value) in doc.checks {
        let table = match value {
            toml::Value::Table(t) => t,
            _ => {
                return Err(ConfigError::UnsupportedValue {
                    path: path.to_path_buf(),
                    check_id,
                    key: "<root>".to_string(),
                    reason: "expected a table of options",
                });
            }
        };

        let mut options: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        for (key, val) in table {
            // Meta-keys are extracted into their own slots, not passed
            // through to per-check option validation (which would
            // reject them as UnknownKey).
            if key == "severity" {
                let s = match &val {
                    toml::Value::String(s) => s.clone(),
                    other => {
                        return Err(ConfigError::SeverityNotString {
                            path: path.to_path_buf(),
                            check_id: check_id.clone(),
                            got: toml_kind(other),
                        });
                    }
                };
                let sev = Severity::parse(&s).map_err(|source| ConfigError::BadSeverity {
                    path: path.to_path_buf(),
                    check_id: check_id.clone(),
                    source,
                })?;
                severity_overrides.insert(check_id.clone(), sev);
                continue;
            }
            if META_KEYS.contains(&key.as_str()) {
                // `enabled` (and any future placeholders) — silently
                // accepted, no behaviour wired today.
                continue;
            }
            let raw = toml_to_raw(&val).ok_or_else(|| ConfigError::UnsupportedValue {
                path: path.to_path_buf(),
                check_id: check_id.clone(),
                key: key.clone(),
                reason: "expected bool, integer, string, or array of those",
            })?;
            options.insert(key, raw);
        }
        checks.insert(check_id, options);
    }

    let layers = if doc.layers.is_empty() {
        None
    } else {
        Some(parse_layers(path, doc.layers)?)
    };

    let config_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let plugins = doc
        .plugins
        .into_iter()
        .map(|p| {
            let pb = PathBuf::from(&p);
            if pb.is_absolute() {
                pb
            } else {
                config_dir.join(pb)
            }
        })
        .collect();

    let overrides = compile_overrides(path, doc.overrides)?;

    Ok(ProjectConfig {
        checks,
        severity_overrides,
        layers,
        plugins,
        invariants: None,
        layers_double_declaration: false,
        engine_type_aware: doc.engine.type_aware,
        overrides,
    })
}

/// Forward-slash, lowercase-on-Windows path key. Mirrors the
/// `path_key` helpers in the checks crate so override globs match the
/// engine's absolute, forward-slashed file keys.
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Compile `[[overrides]]` tables into matchable [`OverrideBlock`]s.
/// Glob patterns are normalised to forward slashes and compiled with
/// `literal_separator(true)` (so `*` doesn't cross `/`), matching the
/// `[public_api].exports` convention. An individual invalid glob is
/// skipped rather than failing the whole config — consistent with
/// public-API glob handling. Per-check option/severity/disabled values
/// are validated for shape here; type-checking option values against
/// each check's schema happens in `Engine::with_config`.
#[allow(clippy::result_large_err)]
fn compile_overrides(
    path: &Path,
    tables: Vec<OverrideTable>,
) -> Result<Vec<OverrideBlock>, ConfigError> {
    use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let config_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let root_abs = std::path::absolute(&config_dir).unwrap_or(config_dir);
    let root_key = path_key(&root_abs);

    let mut out = Vec::with_capacity(tables.len());
    for table in tables {
        let mut builder = GlobSetBuilder::new();
        for raw in &table.paths {
            let normalised = raw.trim_start_matches("./").replace('\\', "/");
            if let Ok(g) = GlobBuilder::new(&normalised)
                .literal_separator(true)
                .build()
            {
                builder.add(g);
            }
            // Invalid glob syntax is skipped silently (a bad pattern
            // simply matches nothing) — same as [public_api].exports.
        }
        let globset = builder.build().unwrap_or_else(|_| GlobSet::empty());

        let mut checks: BTreeMap<String, OverrideCheck> = BTreeMap::new();
        for (check_id, value) in table.checks {
            let oc = parse_override_check(path, &check_id, value)?;
            checks.insert(check_id, oc);
        }

        out.push(OverrideBlock {
            paths: table.paths,
            globset,
            root_key: root_key.clone(),
            checks,
        });
    }
    Ok(out)
}

/// Parse one `[overrides.checks."X"]` sub-table into an
/// [`OverrideCheck`]. `severity` and `disabled` are meta-keys; every
/// other key is a per-check option value (validated against the
/// check's schema later, in the engine).
#[allow(clippy::result_large_err)]
fn parse_override_check(
    path: &Path,
    check_id: &str,
    value: toml::Value,
) -> Result<OverrideCheck, ConfigError> {
    let table = match value {
        toml::Value::Table(t) => t,
        _ => {
            return Err(ConfigError::UnsupportedValue {
                path: path.to_path_buf(),
                check_id: check_id.to_string(),
                key: "<root>".to_string(),
                reason: "expected a table of options",
            });
        }
    };
    let mut oc = OverrideCheck::default();
    for (key, val) in table {
        match key.as_str() {
            "severity" => {
                let s = match &val {
                    toml::Value::String(s) => s.clone(),
                    other => {
                        return Err(ConfigError::SeverityNotString {
                            path: path.to_path_buf(),
                            check_id: check_id.to_string(),
                            got: toml_kind(other),
                        });
                    }
                };
                let sev = Severity::parse(&s).map_err(|source| ConfigError::BadSeverity {
                    path: path.to_path_buf(),
                    check_id: check_id.to_string(),
                    source,
                })?;
                oc.severity = Some(sev);
            }
            "disabled" => match &val {
                toml::Value::Boolean(b) => oc.disabled = Some(*b),
                _ => {
                    return Err(ConfigError::UnsupportedValue {
                        path: path.to_path_buf(),
                        check_id: check_id.to_string(),
                        key: "disabled".to_string(),
                        reason: "expected a boolean",
                    });
                }
            },
            // `enabled` stays a forward-compatible placeholder here too,
            // for parity with the global [checks."X"] block.
            "enabled" => {}
            _ => {
                let raw = toml_to_raw(&val).ok_or_else(|| ConfigError::UnsupportedValue {
                    path: path.to_path_buf(),
                    check_id: check_id.to_string(),
                    key: key.clone(),
                    reason: "expected bool, integer, string, or array of those",
                })?;
                oc.options.insert(key, raw);
            }
        }
    }
    Ok(oc)
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
#[derive(Debug, Default)]
pub struct LoadDiagnostics {
    /// Warning to emit (or empty). Not an error — the load still
    /// succeeded with whatever was loadable.
    pub warnings: Vec<String>,
}

/// Load both `cofferdam.toml` and `cofferdam.invariants.toml`,
/// merging their `[layers]` blocks (invariants takes precedence) and
/// returning a unified `ProjectConfig` plus the path the config was
/// loaded from. Honours `no_config = true` to disable discovery
/// entirely (the `--no-config` CLI flag). Non-fatal load failures
/// (e.g. an unreadable explicit path) surface in `LoadDiagnostics`
/// rather than the `Result` so the CLI can warn-and-continue.
#[allow(clippy::result_large_err)]
pub fn resolve_with_invariants(
    explicit_toml: Option<&Path>,
    cwd: &Path,
    no_config: bool,
) -> Result<(Option<ProjectConfig>, Option<PathBuf>, LoadDiagnostics), ConfigError> {
    let mut diags = LoadDiagnostics::default();
    if no_config {
        return Ok((None, None, diags));
    }

    let toml_path = match explicit_toml {
        Some(p) => Some(p.to_path_buf()),
        None => discover(cwd),
    };

    let (mut cfg, returned_path) = match toml_path {
        Some(p) => match load(&p) {
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
) -> Result<Option<PathBuf>, ConfigError> {
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

#[allow(clippy::result_large_err)]
fn parse_layers(
    path: &Path,
    raw: BTreeMap<String, toml::Value>,
) -> Result<LayersConfig, ConfigError> {
    let mut layers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut allow: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, val) in raw {
        if key == "allow" {
            let table = match val {
                toml::Value::Table(t) => t,
                other => {
                    return Err(ConfigError::UnsupportedValue {
                        path: path.to_path_buf(),
                        check_id: "[layers].allow".to_string(),
                        key: "<root>".to_string(),
                        reason: layer_kind_message(&other),
                    });
                }
            };
            for (layer_name, deps) in table {
                let arr = match deps {
                    toml::Value::Array(a) => a,
                    other => {
                        return Err(ConfigError::UnsupportedValue {
                            path: path.to_path_buf(),
                            check_id: format!("[layers].allow.{}", layer_name),
                            key: "<value>".to_string(),
                            reason: layer_kind_message(&other),
                        });
                    }
                };
                let mut names = Vec::with_capacity(arr.len());
                for item in arr {
                    match item {
                        toml::Value::String(s) => names.push(s),
                        other => {
                            return Err(ConfigError::UnsupportedValue {
                                path: path.to_path_buf(),
                                check_id: format!("[layers].allow.{}", layer_name),
                                key: "<element>".to_string(),
                                reason: layer_kind_message(&other),
                            });
                        }
                    }
                }
                allow.insert(layer_name, names);
            }
            continue;
        }

        let arr = match val {
            toml::Value::Array(a) => a,
            other => {
                return Err(ConfigError::UnsupportedValue {
                    path: path.to_path_buf(),
                    check_id: format!("[layers].{}", key),
                    key: "<value>".to_string(),
                    reason: layer_kind_message(&other),
                });
            }
        };
        let mut globs = Vec::with_capacity(arr.len());
        for item in arr {
            match item {
                toml::Value::String(s) => globs.push(s),
                other => {
                    return Err(ConfigError::UnsupportedValue {
                        path: path.to_path_buf(),
                        check_id: format!("[layers].{}", key),
                        key: "<element>".to_string(),
                        reason: layer_kind_message(&other),
                    });
                }
            }
        }
        layers.insert(key, globs);
    }
    let project_root = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(LayersConfig {
        project_root,
        layers,
        allow,
    })
}

fn layer_kind_message(v: &toml::Value) -> &'static str {
    match v {
        toml::Value::Table(_) => "expected an array of layer-name strings",
        toml::Value::Array(_) => "expected a string layer name",
        _ => "expected an array of glob strings or a string",
    }
}

fn toml_kind(v: &toml::Value) -> &'static str {
    match v {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "bool",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
        toml::Value::Datetime(_) => "datetime",
    }
}

/// Convert a `toml::Value` to a `RawOptionValue`. `None` for any TOML
/// type the option schema can't represent (Float, Datetime, nested
/// Table). Caller turns `None` into `ConfigError::UnsupportedValue`.
fn toml_to_raw(v: &toml::Value) -> Option<RawOptionValue> {
    match v {
        toml::Value::Boolean(b) => Some(RawOptionValue::Bool(*b)),
        toml::Value::Integer(i) => Some(RawOptionValue::Int(*i)),
        toml::Value::String(s) => Some(RawOptionValue::String(s.clone())),
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(toml_to_raw(item)?);
            }
            Some(RawOptionValue::List(out))
        }
        // Floats, datetimes, and nested tables aren't representable in
        // the option schema today. Adding any of these is a deliberate
        // design choice elsewhere; surface a clear error rather than
        // silently coercing.
        _ => None,
    }
}

/// Apply a project config to a check schema. Returns the validated
/// `CheckOptions` for that one check, falling back to schema defaults
/// when the config doesn't mention this check ID.
#[allow(clippy::result_large_err)]
pub fn options_for(
    project: &ProjectConfig,
    config_path: &Path,
    check_id: &str,
    schema: &[cofferdam_core::OptionSpec],
) -> Result<CheckOptions, ConfigError> {
    match project.checks.get(check_id) {
        Some(raw) => options_for_raw(config_path, check_id, schema, raw),
        None => Ok(CheckOptions::defaults_from(schema)),
    }
}

/// Validate an already-assembled raw option bag against a check's
/// schema. Shared by [`options_for`] (global `[checks."X"]`) and the
/// engine's per-glob override path (cd-m5tu), so both surface the same
/// `MisplacedTopLevelKey` / `Validate` diagnostics.
#[allow(clippy::result_large_err)]
pub fn options_for_raw(
    config_path: &Path,
    check_id: &str,
    schema: &[cofferdam_core::OptionSpec],
    raw: &BTreeMap<String, RawOptionValue>,
) -> Result<CheckOptions, ConfigError> {
    validate_options(check_id, schema, raw).map_err(|source| {
        // When `validate_options` rejects a key that looks like a
        // well-known top-level cofferdam.toml key (plugins, extends,
        // include), the real cause is TOML lexical scoping — the user
        // wrote the key after a `[checks."X"]` header and TOML
        // silently nested it there. Emit a directive error so they
        // know to move it to the top of the file, rather than
        // presenting a confusing "check does not declare option '...'"
        // message.
        if let OptionsError::UnknownKey { ref key, .. } = source {
            if MISPLACED_TOP_LEVEL_KEYS.contains(&key.as_str()) {
                return ConfigError::MisplacedTopLevelKey {
                    path: config_path.to_path_buf(),
                    intended_key: key.clone(),
                    nested_under: check_id.to_string(),
                };
            }
        }
        ConfigError::Validate {
            path: config_path.to_path_buf(),
            check_id: check_id.to_string(),
            source,
        }
    })
}

/// Return the set of check IDs that the config references but aren't in
/// the registered list. The CLI surfaces these as warnings — typos
/// shouldn't break a build, but should be visible.
pub fn unknown_check_ids<'a>(project: &'a ProjectConfig, registered: &[&str]) -> Vec<&'a String> {
    project
        .checks
        .keys()
        .filter(|id| !registered.iter().any(|r| r == &id.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{OptionDefault, OptionKind, OptionSpec};
    use std::fs;
    use tempfile::tempdir;

    const SCHEMA: &[OptionSpec] = &[OptionSpec {
        name: "limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(80),
        doc: "max line length",
    }];

    #[test]
    fn parse_minimal_config() {
        let raw = r#"
[checks."Readability.MaxLineLength"]
limit = 120
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        let bag = cfg
            .checks
            .get("Readability.MaxLineLength")
            .expect("present");
        assert_eq!(bag.get("limit"), Some(&RawOptionValue::Int(120)));
    }

    #[test]
    fn engine_type_aware_defaults_to_enabled() {
        // No [engine] table → None → enabled.
        let cfg = parse(Path::new("test.toml"), "").expect("parse");
        assert_eq!(cfg.engine_type_aware, None);
        assert!(cfg.type_aware_enabled());
    }

    #[test]
    fn engine_type_aware_false_opts_out() {
        let raw = r#"
[engine]
type_aware = false
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_type_aware, Some(false));
        assert!(!cfg.type_aware_enabled());
    }

    #[test]
    fn engine_type_aware_true_is_enabled() {
        let raw = r#"
[engine]
type_aware = true
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_type_aware, Some(true));
        assert!(cfg.type_aware_enabled());
    }

    #[test]
    fn engine_unknown_keys_are_ignored() {
        // Forward-compat: an unrecognised [engine] key must not fail the
        // parse (the table grows additively).
        let raw = r#"
[engine]
type_aware = false
future_toggle = 42
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_type_aware, Some(false));
    }

    #[test]
    fn meta_keys_are_separated_from_options() {
        let raw = r#"
[checks."Readability.MaxLineLength"]
limit = 120
severity = "high"
enabled = true
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        let bag = cfg
            .checks
            .get("Readability.MaxLineLength")
            .expect("present");
        // `limit` flows through to the per-check option bag; `severity`
        // goes to `severity_overrides`; `enabled` is silently accepted
        // (no behaviour wired today).
        assert_eq!(bag.len(), 1);
        assert!(bag.contains_key("limit"));
        assert!(!bag.contains_key("severity"));
        assert!(!bag.contains_key("enabled"));
        assert_eq!(
            cfg.severity_overrides.get("Readability.MaxLineLength"),
            Some(&Severity::High)
        );
    }

    #[test]
    fn parse_overrides_block() {
        let raw = r#"
[[overrides]]
paths = ["**/*.test.ts", "**/*.test.tsx"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 400
[overrides.checks."Design.OrphanExport"]
disabled = true
severity = "info"
"#;
        let cfg = parse(Path::new("cofferdam.toml"), raw).expect("parse");
        assert_eq!(cfg.overrides.len(), 1);
        let block = &cfg.overrides[0];
        assert_eq!(block.paths, vec!["**/*.test.ts", "**/*.test.tsx"]);

        let mfl = block
            .checks
            .get("Readability.MaxFunctionLength")
            .expect("mfl override present");
        assert_eq!(mfl.options.get("limit"), Some(&RawOptionValue::Int(400)));
        assert_eq!(mfl.disabled, None, "limit-only override sets no disabled");
        assert_eq!(mfl.severity, None);

        let orphan = block
            .checks
            .get("Design.OrphanExport")
            .expect("orphan override present");
        assert_eq!(orphan.disabled, Some(true));
        assert_eq!(orphan.severity, Some(Severity::Info));
        assert!(orphan.options.is_empty());
    }

    #[test]
    fn parse_multiple_override_blocks_preserve_order() {
        let raw = r#"
[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 200

[[overrides]]
paths = ["src/legacy/**"]
[overrides.checks."Refactor.CyclomaticComplexity"]
severity = "info"
"#;
        let cfg = parse(Path::new("cofferdam.toml"), raw).expect("parse");
        assert_eq!(cfg.overrides.len(), 2);
        assert_eq!(cfg.overrides[0].paths, vec!["**/*.test.ts"]);
        assert_eq!(cfg.overrides[1].paths, vec!["src/legacy/**"]);
    }

    #[test]
    fn override_glob_matches_relative_to_root() {
        // is_match must reduce an absolute, engine-promoted path to the
        // project-relative form the globs are written against.
        let raw = r#"
[[overrides]]
paths = ["**/*.test.tsx"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 400
"#;
        // Use a path the test controls so root absolutization is stable.
        let cfg = parse(Path::new("cofferdam.toml"), raw).expect("parse");
        let block = &cfg.overrides[0];
        let root = &block.root_key; // absolute, normalised
        assert!(block.is_match(&format!("{root}/src/Lobby.test.tsx")));
        assert!(block.is_match(&format!("{root}/Lobby.test.tsx")));
        assert!(!block.is_match(&format!("{root}/src/Lobby.tsx")));
    }

    #[test]
    fn override_bad_severity_is_rejected() {
        let raw = r#"
[[overrides]]
paths = ["**/*.ts"]
[overrides.checks."X.Y"]
severity = "extreme"
"#;
        let err = parse(Path::new("cofferdam.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::BadSeverity { .. }));
    }

    #[test]
    fn override_non_bool_disabled_is_rejected() {
        let raw = r#"
[[overrides]]
paths = ["**/*.ts"]
[overrides.checks."X.Y"]
disabled = "yes"
"#;
        let err = parse(Path::new("cofferdam.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedValue { .. }));
    }

    #[test]
    fn no_overrides_yields_empty_vec() {
        let cfg = parse(Path::new("cofferdam.toml"), "").expect("parse");
        assert!(cfg.overrides.is_empty());
    }

    #[test]
    fn unsupported_value_errors() {
        let raw = r#"
[checks."Foo.Bar"]
weird = 1.5
"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedValue { .. }));
    }

    #[test]
    fn bad_severity_string_is_rejected() {
        let raw = r#"
[checks."X.Y"]
severity = "extreme"
"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::BadSeverity { .. }));
    }

    #[test]
    fn non_string_severity_is_rejected() {
        let raw = r#"
[checks."X.Y"]
severity = 5
"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::SeverityNotString { .. }));
    }

    #[test]
    fn missing_top_level_checks_yields_empty_config() {
        let cfg = parse(Path::new("test.toml"), "").expect("parse");
        assert!(cfg.checks.is_empty());
    }

    #[test]
    fn options_for_uses_overrides() {
        let mut bag = BTreeMap::new();
        bag.insert("limit".to_string(), RawOptionValue::Int(120));
        let mut checks = BTreeMap::new();
        checks.insert("Readability.MaxLineLength".to_string(), bag);
        let project = ProjectConfig {
            checks,
            severity_overrides: BTreeMap::new(),
            layers: None,
            plugins: Vec::new(),
            invariants: None,
            layers_double_declaration: false,
            engine_type_aware: None,
            overrides: Vec::new(),
        };

        let opts = options_for(
            &project,
            Path::new("test.toml"),
            "Readability.MaxLineLength",
            SCHEMA,
        )
        .expect("validate");
        assert_eq!(opts.get_int("limit"), Some(120));
    }

    #[test]
    fn options_for_unknown_check_uses_defaults() {
        let project = ProjectConfig::default();
        let opts =
            options_for(&project, Path::new("test.toml"), "Missing.Check", SCHEMA).expect("ok");
        assert_eq!(opts.get_int("limit"), Some(80));
    }

    #[test]
    fn options_for_validation_error_propagates() {
        let mut bag = BTreeMap::new();
        bag.insert("limit".to_string(), RawOptionValue::String("nope".into()));
        let mut checks = BTreeMap::new();
        checks.insert("X.Y".to_string(), bag);
        let project = ProjectConfig {
            checks,
            severity_overrides: BTreeMap::new(),
            layers: None,
            plugins: Vec::new(),
            invariants: None,
            layers_double_declaration: false,
            engine_type_aware: None,
            overrides: Vec::new(),
        };

        let err = options_for(&project, Path::new("test.toml"), "X.Y", SCHEMA).unwrap_err();
        assert!(matches!(err, ConfigError::Validate { .. }));
    }

    #[test]
    fn unknown_check_ids_lists_strays() {
        let mut checks = BTreeMap::new();
        checks.insert("Readability.MaxLineLength".to_string(), BTreeMap::new());
        checks.insert("Bogus.NotReal".to_string(), BTreeMap::new());
        let project = ProjectConfig {
            checks,
            severity_overrides: BTreeMap::new(),
            layers: None,
            plugins: Vec::new(),
            invariants: None,
            layers_double_declaration: false,
            engine_type_aware: None,
            overrides: Vec::new(),
        };

        let registered = ["Readability.MaxLineLength"];
        let unknown = unknown_check_ids(&project, &registered);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0], "Bogus.NotReal");
    }

    #[test]
    fn discover_walks_up_to_find_config() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).expect("create dirs");
        let cfg_path = dir.path().join(FILE_NAME);
        fs::write(&cfg_path, "").expect("write config");

        let found = discover(&nested).expect("found");
        // Compare canonicalised paths — tempdir on macOS lives under /var
        // which symlinks to /private/var; canonicalise both sides.
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&cfg_path).unwrap()
        );
    }

    #[test]
    fn discover_stops_at_git_root() {
        let dir = tempdir().expect("tempdir");
        let outer_cfg = dir.path().join(FILE_NAME);
        fs::write(&outer_cfg, "").expect("write outer");

        // .git below the outer config — discover() should stop here and
        // not return the outer config.
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).expect("create .git");
        let nested = repo.join("src");
        fs::create_dir_all(&nested).expect("create nested");

        assert_eq!(discover(&nested), None);
    }

    #[test]
    fn discover_finds_inside_repo() {
        let dir = tempdir().expect("tempdir");
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).expect("create .git");
        let cfg_path = repo.join(FILE_NAME);
        fs::write(&cfg_path, "").expect("write config");
        let nested = repo.join("src/deep");
        fs::create_dir_all(&nested).expect("create nested");

        let found = discover(&nested).expect("found");
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(&cfg_path).unwrap()
        );
    }

    // --- MisplacedTopLevelKey detection tests ---

    /// Helper: build a ProjectConfig that has `key` as an unknown option
    /// nested under the given check_id, then call options_for with an
    /// empty schema (so every key is unknown).
    fn options_for_with_raw_key(check_id: &str, key: &str, val: RawOptionValue) -> ConfigError {
        let mut bag = BTreeMap::new();
        bag.insert(key.to_string(), val);
        let mut checks = BTreeMap::new();
        checks.insert(check_id.to_string(), bag);
        let project = ProjectConfig {
            checks,
            severity_overrides: BTreeMap::new(),
            layers: None,
            plugins: Vec::new(),
            invariants: None,
            layers_double_declaration: false,
            engine_type_aware: None,
            overrides: Vec::new(),
        };
        // Empty schema → every key is unknown to validate_options.
        options_for(&project, Path::new("test.toml"), check_id, &[]).unwrap_err()
    }

    #[test]
    fn misplaced_plugins_after_checks_table_emits_directive_error() {
        // Simulates:
        //   [checks."Readability.MaxLineLength"]
        //   limit = 120
        //   plugins = ["./my-plugin.mjs"]
        //
        // The parse() step strips `limit` into the raw bag before
        // reaching options_for, but `plugins` (an array) would be kept
        // as-is if it weren't in META_KEYS — so we test options_for
        // directly with `plugins` in the raw bag.
        let err = options_for_with_raw_key(
            "Readability.MaxLineLength",
            "plugins",
            RawOptionValue::List(vec![RawOptionValue::String("./my-plugin.mjs".into())]),
        );

        assert!(
            matches!(
                &err,
                ConfigError::MisplacedTopLevelKey {
                    ref intended_key,
                    ref nested_under,
                    ..
                } if intended_key == "plugins" && nested_under == "Readability.MaxLineLength"
            ),
            "expected MisplacedTopLevelKey, got: {err:?}"
        );

        // Display must mention the key and the directive word "Move".
        let msg = err.to_string();
        assert!(msg.contains("plugins"), "missing key name: {msg}");
        assert!(
            msg.contains("Move") || msg.contains("move"),
            "missing directive: {msg}"
        );
        assert!(msg.contains("[checks."), "missing table hint: {msg}");
    }

    #[test]
    fn misplaced_extends_emits_directive_error() {
        let err = options_for_with_raw_key(
            "Refactor.CyclomaticComplexity",
            "extends",
            RawOptionValue::String("./base.toml".into()),
        );
        assert!(
            matches!(&err, ConfigError::MisplacedTopLevelKey { ref intended_key, .. } if intended_key == "extends"),
            "expected MisplacedTopLevelKey for extends, got: {err:?}"
        );
    }

    #[test]
    fn misplaced_include_emits_directive_error() {
        let err = options_for_with_raw_key(
            "Design.MaxParameters",
            "include",
            RawOptionValue::List(vec![RawOptionValue::String("src/**".into())]),
        );
        assert!(
            matches!(&err, ConfigError::MisplacedTopLevelKey { ref intended_key, .. } if intended_key == "include"),
            "expected MisplacedTopLevelKey for include, got: {err:?}"
        );
    }

    #[test]
    fn unknown_check_option_typo_still_errors_normally() {
        // Regression: a genuine typo in a checks table option (limitt = 120)
        // must still produce ConfigError::Validate, not MisplacedTopLevelKey.
        let err = options_for_with_raw_key(
            "Readability.MaxLineLength",
            "limitt",
            RawOptionValue::Int(120),
        );
        assert!(
            matches!(err, ConfigError::Validate { .. }),
            "expected Validate for a typo'd option key, got: {err:?}"
        );
    }
}
