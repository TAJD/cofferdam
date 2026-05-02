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

use cofferdam_core::{validate_options, CheckOptions, OptionsError, RawOptionValue};
use serde::Deserialize;

/// Filename loaders look for during walk-up discovery.
pub const FILE_NAME: &str = "cofferdam.toml";

/// Meta-keys that may appear inside `[checks."X.Y"]` blocks but aren't
/// per-check options. They're forward-compatible placeholders for cd-t1a
/// (severity gating). Stripped before per-check option validation; a
/// value present here today is silently accepted but does not yet alter
/// behaviour.
const META_KEYS: &[&str] = &["severity", "enabled"];

/// Parsed project config: per-check raw option bags, plus a list of any
/// check IDs that the config referenced but the engine does not have
/// registered (so the CLI can surface a friendly warning without
/// failing the build over a typo).
#[derive(Debug, Clone, Default)]
pub struct ProjectConfig {
    pub checks: BTreeMap<String, BTreeMap<String, RawOptionValue>>,
}

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
    #[error("config {path}: option validation failed for [checks.\"{check_id}\"]: {source}")]
    Validate {
        path: PathBuf,
        check_id: String,
        #[source]
        source: OptionsError,
    },
}

/// TOML document layout. Anchored to a single top-level `[checks]` table
/// so we have an obvious place to grow future top-level sections later
/// (e.g. `[output]`, `[discovery]`) without breaking existing files.
#[derive(Debug, Deserialize, Default)]
struct TomlDoc {
    #[serde(default)]
    checks: BTreeMap<String, toml::Value>,
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
            // Meta-keys (severity, enabled) are accepted today as
            // forward-compatible placeholders for cd-t1a but stripped
            // before per-check option validation so they don't trigger
            // an UnknownKey error.
            if META_KEYS.contains(&key.as_str()) {
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

    Ok(ProjectConfig { checks })
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
        Some(raw) => {
            validate_options(check_id, schema, raw).map_err(|source| ConfigError::Validate {
                path: config_path.to_path_buf(),
                check_id: check_id.to_string(),
                source,
            })
        }
        None => Ok(CheckOptions::defaults_from(schema)),
    }
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
    fn meta_keys_are_stripped() {
        let raw = r#"
[checks."Readability.MaxLineLength"]
limit = 120
severity = "warning"
enabled = true
"#;
        let cfg = parse(Path::new("test.toml"), raw).expect("parse");
        let bag = cfg
            .checks
            .get("Readability.MaxLineLength")
            .expect("present");
        assert_eq!(bag.len(), 1);
        assert!(bag.contains_key("limit"));
        assert!(!bag.contains_key("severity"));
        assert!(!bag.contains_key("enabled"));
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
        let project = ProjectConfig { checks };

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
        let project = ProjectConfig { checks };

        let err = options_for(&project, Path::new("test.toml"), "X.Y", SCHEMA).unwrap_err();
        assert!(matches!(err, ConfigError::Validate { .. }));
    }

    #[test]
    fn unknown_check_ids_lists_strays() {
        let mut checks = BTreeMap::new();
        checks.insert("Readability.MaxLineLength".to_string(), BTreeMap::new());
        checks.insert("Bogus.NotReal".to_string(), BTreeMap::new());
        let project = ProjectConfig { checks };

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
}
