//! Option validation, merging, and layers parsing.

use std::collections::BTreeMap;
use std::path::Path;

use cofferdam_core::graph::LayersConfig;
use cofferdam_core::{validate_options, CheckOptions, OptionsError, RawOptionValue};

use super::{ConfigError, ProjectConfig};

/// Top-level `cofferdam.toml` keys that users sometimes accidentally nest
/// under a `[checks."X.Y"]` table because TOML's lexical scoping assigns
/// all keys after a table header to that table. When one of these appears
/// as an unknown option, we emit a directive error instead of the generic
/// "check does not declare option '...'" message.
const MISPLACED_TOP_LEVEL_KEYS: &[&str] = &["plugins", "extends", "include"];

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
    // `enabled` was historically stripped by the config loader for every
    // check, as a forward-compatible placeholder. That made
    // `Refactor.PurityHeuristic` — whose sole option is called `enabled`
    // — impossible to turn on (CD-324). The loader now passes it
    // through; drop it here only for the checks that do not declare it,
    // so configs written against the old placeholder keep loading.
    let stripped;
    let raw = if raw.contains_key("enabled") && !schema.iter().any(|s| s.name == "enabled") {
        stripped = raw
            .iter()
            .filter(|(k, _)| k.as_str() != "enabled")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>();
        &stripped
    } else {
        raw
    };

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

#[allow(clippy::result_large_err)]
pub fn parse_layers(
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
        .unwrap_or_else(|| std::path::PathBuf::from("."));
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
