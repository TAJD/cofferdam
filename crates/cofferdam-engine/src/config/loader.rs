//! TOML loading, discovery, and deserialization for `cofferdam.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cofferdam_core::{RawOptionValue, Severity};
use serde::Deserialize;

use super::{ConfigError, OverrideBlock, OverrideCheck, ProjectConfig};

/// Filename loaders look for during walk-up discovery.
pub const FILE_NAME: &str = "cofferdam.toml";

/// Meta-keys that may appear inside `[checks."X.Y"]` blocks but aren't
/// per-check options. `severity` (cd-t1a) is now first-class — extracted
/// into `ProjectConfig::severity_overrides` rather than passed through
/// to `validate_options`. `enabled` is still a forward-compatible
/// placeholder (no behaviour wired yet).
const META_KEYS: &[&str] = &["severity", "enabled"];

/// TOML document layout. Top-level sections grow additively here.
#[derive(Debug, Deserialize, Default)]
pub struct TomlDoc {
    #[serde(default)]
    pub checks: BTreeMap<String, toml::Value>,
    /// `[layers]` for `Design.LayerViolation`. Each top-level key is
    /// either a layer name (whose value is a string array of globs) or
    /// the reserved sub-table `allow` mapping layer names to lists of
    /// allowed dependency layers.
    #[serde(default)]
    pub layers: BTreeMap<String, toml::Value>,
    /// `plugins = [...]` — Node-side plugin module paths (cd-7e4 / cd-81a.7).
    /// Resolved relative to the config file's directory.
    #[serde(default)]
    pub plugins: Vec<String>,
    /// `[engine]` — engine-level toggles. Today just `type_aware`
    /// (cd-9hp.2.4); grows additively.
    #[serde(default)]
    pub engine: EngineSection,
    /// `[[overrides]]` — per-path-glob check config (cd-m5tu). Array of
    /// tables, each with `paths = [...]` and a `[overrides.checks."X"]`
    /// sub-table.
    #[serde(default)]
    pub overrides: Vec<OverrideTable>,
}

/// Raw `[[overrides]]` element. `checks` keys are check ids; each value
/// is a table of option/`severity`/`disabled` entries, parsed the same
/// way a `[checks."X"]` block is.
#[derive(Debug, Deserialize, Default)]
pub struct OverrideTable {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub checks: BTreeMap<String, toml::Value>,
}

/// `[engine]` table. Unknown keys are ignored (forward-compatible).
#[derive(Debug, Deserialize, Default)]
pub struct EngineSection {
    /// `type_aware = false` force-disables type-aware checks. Absent
    /// means enabled (the default).
    #[serde(default)]
    pub type_aware: Option<bool>,
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
pub fn parse(path: &Path, raw: &str) -> Result<ProjectConfig, ConfigError> {
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
        layers: None,
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
pub fn compile_overrides(
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
pub fn parse_override_check(
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
pub fn toml_to_raw(v: &toml::Value) -> Option<RawOptionValue> {
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
