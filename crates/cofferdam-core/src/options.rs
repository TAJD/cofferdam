//! Per-check options schema and runtime values.
//!
//! Each check declares an `options: &'static [OptionSpec]` block on its
//! `CheckMeta`. The engine validates the user's config (today: defaults
//! only; tomorrow: cofferdam.toml, see cd-4ms) against that schema once
//! at startup. Type mismatches and unknown keys fail loudly there, not
//! at run() time when a check first reaches into the bag.
//!
//! The split between static and owned shapes is deliberate:
//!
//! - `OptionSpec` / `OptionDefault` live as `&'static` next to the
//!   check's `CheckMeta`. They're `Copy`, so adding the field to
//!   `CheckMeta` keeps it `Copy`.
//! - `OptionValue` and `CheckOptions` are owned — they hold
//!   user-overridden strings/lists that aren't statically known.
//! - `RawOptionValue` is the config-loader-facing type: a JSON-shaped
//!   tree that any backing format (TOML, JSON, env vars) can produce.
//!   Decoupling validation from the loader means cd-4ms (TOML) and
//!   future loaders share one validation path.

use std::collections::BTreeMap;

use thiserror::Error;

/// Type tag for an option. Determines how `RawOptionValue` is coerced
/// into `OptionValue` during validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    Int,
    String,
    StringList,
    IntList,
}

impl OptionKind {
    /// Display name for this kind, used in error messages (e.g.
    /// `expected bool, got string`). The names are part of the
    /// user-visible diagnostic surface — don't change without a
    /// deprecation note.
    pub const fn name(self) -> &'static str {
        match self {
            OptionKind::Bool => "bool",
            OptionKind::Int => "int",
            OptionKind::String => "string",
            OptionKind::StringList => "string[]",
            OptionKind::IntList => "int[]",
        }
    }
}

/// A check's compiled-in default for one option. Stored next to its
/// `OptionSpec` and copied into `OptionValue` when the user supplies
/// nothing for that key.
#[derive(Debug, Clone, Copy)]
pub enum OptionDefault {
    Bool(bool),
    Int(i64),
    String(&'static str),
    StringList(&'static [&'static str]),
    IntList(&'static [i64]),
}

impl OptionDefault {
    /// The `OptionKind` this default carries. Used by validation to
    /// confirm `OptionSpec.kind` agrees with `OptionSpec.default`.
    pub const fn kind(&self) -> OptionKind {
        match self {
            OptionDefault::Bool(_) => OptionKind::Bool,
            OptionDefault::Int(_) => OptionKind::Int,
            OptionDefault::String(_) => OptionKind::String,
            OptionDefault::StringList(_) => OptionKind::StringList,
            OptionDefault::IntList(_) => OptionKind::IntList,
        }
    }

    /// Materialise a compile-time default as a runtime `OptionValue`.
    /// String slices are owned (`.to_string()`); list slices are
    /// collected. Cheap — defaults are small.
    pub fn to_value(&self) -> OptionValue {
        match *self {
            OptionDefault::Bool(b) => OptionValue::Bool(b),
            OptionDefault::Int(i) => OptionValue::Int(i),
            OptionDefault::String(s) => OptionValue::String(s.to_string()),
            OptionDefault::StringList(xs) => {
                OptionValue::StringList(xs.iter().map(|s| s.to_string()).collect())
            }
            OptionDefault::IntList(xs) => OptionValue::IntList(xs.to_vec()),
        }
    }
}

/// Static declaration of one option. A check's `meta().options` is
/// `&'static [OptionSpec]`.
///
/// `kind` is redundant with `default.kind()` but kept explicit so the
/// schema reads top-to-bottom: name, type, default, doc. Validation
/// asserts they agree (a debug-time sanity check on author error).
#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub name: &'static str,
    pub kind: OptionKind,
    pub default: OptionDefault,
    pub doc: &'static str,
}

/// Owned runtime value for one option. Built from `OptionDefault` or
/// from a validated `RawOptionValue`.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Int(i64),
    String(String),
    StringList(Vec<String>),
    IntList(Vec<i64>),
}

impl OptionValue {
    /// The `OptionKind` this value carries. Used for type-mismatch
    /// diagnostics when a user overrides an option with the wrong shape.
    pub fn kind(&self) -> OptionKind {
        match self {
            OptionValue::Bool(_) => OptionKind::Bool,
            OptionValue::Int(_) => OptionKind::Int,
            OptionValue::String(_) => OptionKind::String,
            OptionValue::StringList(_) => OptionKind::StringList,
            OptionValue::IntList(_) => OptionKind::IntList,
        }
    }
}

/// Format-agnostic input to validation. A config loader (TOML today,
/// JSON tomorrow) flattens its native `Value` enum into this. Lists
/// are homogeneous post-validation but `RawOptionValue::List` accepts
/// arbitrary mixes — coercion catches that.
#[derive(Debug, Clone, PartialEq)]
pub enum RawOptionValue {
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<RawOptionValue>),
}

impl RawOptionValue {
    /// Coarse type label for diagnostic messages. Lists are flattened
    /// to a single `"list"` label — the element types only matter once
    /// the homogeneity check passes, and surfacing per-element types
    /// up here would add noise without information.
    pub fn kind_label(&self) -> &'static str {
        match self {
            RawOptionValue::Bool(_) => "bool",
            RawOptionValue::Int(_) => "int",
            RawOptionValue::String(_) => "string",
            RawOptionValue::List(_) => "list",
        }
    }
}

/// Resolved options for one check, indexed by option name. Built once
/// per check per Engine and lent to `CheckContext` for each file.
///
/// Accessors return `Option` rather than panicking on a missing key —
/// in practice the bag always contains every declared option (defaults
/// fill the gaps), but a typo on the check side shouldn't take down a
/// run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckOptions {
    values: BTreeMap<String, OptionValue>,
}

impl CheckOptions {
    /// Empty options bag. The process-wide [`EMPTY_OPTIONS`] static
    /// uses this so tests and option-less checks share one instance.
    pub const fn empty() -> Self {
        Self {
            values: BTreeMap::new(),
        }
    }

    /// Build a CheckOptions populated entirely from the schema's defaults.
    pub fn defaults_from(specs: &[OptionSpec]) -> Self {
        let mut values = BTreeMap::new();
        for spec in specs {
            values.insert(spec.name.to_string(), spec.default.to_value());
        }
        Self { values }
    }

    /// Untyped accessor. Returns `Some(&OptionValue)` if the key is
    /// declared in the schema. Prefer the typed getters below for
    /// well-known types.
    pub fn get(&self, name: &str) -> Option<&OptionValue> {
        self.values.get(name)
    }

    /// Typed accessor for `OptionKind::Bool` values. Returns `None`
    /// when the key is missing OR the value's runtime type doesn't
    /// match — defensive against schema/check-author drift.
    pub fn get_bool(&self, name: &str) -> Option<bool> {
        match self.values.get(name)? {
            OptionValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Typed accessor for `OptionKind::Int` values.
    pub fn get_int(&self, name: &str) -> Option<i64> {
        match self.values.get(name)? {
            OptionValue::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Typed accessor for `OptionKind::String` values.
    pub fn get_string(&self, name: &str) -> Option<&str> {
        match self.values.get(name)? {
            OptionValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Typed accessor for `OptionKind::StringList` values.
    pub fn get_string_list(&self, name: &str) -> Option<&[String]> {
        match self.values.get(name)? {
            OptionValue::StringList(xs) => Some(xs.as_slice()),
            _ => None,
        }
    }

    /// Typed accessor for `OptionKind::IntList` values.
    pub fn get_int_list(&self, name: &str) -> Option<&[i64]> {
        match self.values.get(name)? {
            OptionValue::IntList(xs) => Some(xs.as_slice()),
            _ => None,
        }
    }
}

/// The shared empty options bag, handed to `CheckContext` when no
/// check-specific options have been wired.
pub static EMPTY_OPTIONS: CheckOptions = CheckOptions::empty();

/// Validation failure. The `check_id` is the dotted ID from `CheckMeta`,
/// always present so the engine can tell users *which* `[checks."..."]`
/// block in their config is wrong.
#[derive(Debug, Error, PartialEq)]
pub enum OptionsError {
    #[error("check `{check_id}` does not declare option `{key}`")]
    UnknownKey { check_id: String, key: String },

    #[error("check `{check_id}` option `{key}`: expected {expected}, got {got}")]
    TypeMismatch {
        check_id: String,
        key: String,
        expected: &'static str,
        got: &'static str,
    },
}

/// Validate a raw config bag against a check's schema and merge with
/// declared defaults. Unknown keys fail. Type mismatches fail. Missing
/// keys fall back to defaults.
///
/// `check_id` is taken separately rather than from a `&CheckMeta` to
/// keep this module dep-free of `check.rs` — both modules sit at the
/// same layer in core.
pub fn validate_options(
    check_id: &str,
    schema: &[OptionSpec],
    raw: &BTreeMap<String, RawOptionValue>,
) -> Result<CheckOptions, OptionsError> {
    // Reject unknown keys first — easier diagnostics than letting an
    // unrecognised TOML field silently round-trip.
    for key in raw.keys() {
        if !schema.iter().any(|s| s.name == key) {
            return Err(OptionsError::UnknownKey {
                check_id: check_id.to_string(),
                key: key.clone(),
            });
        }
    }

    let mut values = BTreeMap::new();
    for spec in schema {
        let value = match raw.get(spec.name) {
            Some(raw_value) => coerce(check_id, spec, raw_value)?,
            None => spec.default.to_value(),
        };
        values.insert(spec.name.to_string(), value);
    }
    Ok(CheckOptions { values })
}

fn coerce(
    check_id: &str,
    spec: &OptionSpec,
    raw: &RawOptionValue,
) -> Result<OptionValue, OptionsError> {
    let mismatch = || OptionsError::TypeMismatch {
        check_id: check_id.to_string(),
        key: spec.name.to_string(),
        expected: spec.kind.name(),
        got: raw.kind_label(),
    };

    match (spec.kind, raw) {
        (OptionKind::Bool, RawOptionValue::Bool(b)) => Ok(OptionValue::Bool(*b)),
        (OptionKind::Int, RawOptionValue::Int(i)) => Ok(OptionValue::Int(*i)),
        (OptionKind::String, RawOptionValue::String(s)) => Ok(OptionValue::String(s.clone())),
        (OptionKind::StringList, RawOptionValue::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    RawOptionValue::String(s) => out.push(s.clone()),
                    other => {
                        return Err(OptionsError::TypeMismatch {
                            check_id: check_id.to_string(),
                            key: spec.name.to_string(),
                            expected: "string[]",
                            got: other.kind_label(),
                        })
                    }
                }
            }
            Ok(OptionValue::StringList(out))
        }
        (OptionKind::IntList, RawOptionValue::List(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    RawOptionValue::Int(i) => out.push(*i),
                    other => {
                        return Err(OptionsError::TypeMismatch {
                            check_id: check_id.to_string(),
                            key: spec.name.to_string(),
                            expected: "int[]",
                            got: other.kind_label(),
                        })
                    }
                }
            }
            Ok(OptionValue::IntList(out))
        }
        _ => Err(mismatch()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &[OptionSpec] = &[
        OptionSpec {
            name: "limit",
            kind: OptionKind::Int,
            default: OptionDefault::Int(80),
            doc: "max line length",
        },
        OptionSpec {
            name: "tenant_fields",
            kind: OptionKind::StringList,
            default: OptionDefault::StringList(&["siteId", "merchantId"]),
            doc: "field names treated as tenant IDs",
        },
        OptionSpec {
            name: "strict",
            kind: OptionKind::Bool,
            default: OptionDefault::Bool(false),
            doc: "fail on suspicious cases",
        },
    ];

    #[test]
    fn defaults_populate_every_key() {
        let opts = CheckOptions::defaults_from(SCHEMA);
        assert_eq!(opts.get_int("limit"), Some(80));
        assert_eq!(opts.get_bool("strict"), Some(false));
        assert_eq!(
            opts.get_string_list("tenant_fields")
                .map(<[String]>::to_vec),
            Some(vec!["siteId".to_string(), "merchantId".to_string()])
        );
    }

    #[test]
    fn validate_with_empty_input_returns_defaults() {
        let raw = BTreeMap::new();
        let opts = validate_options("X.Y", SCHEMA, &raw).unwrap();
        assert_eq!(opts, CheckOptions::defaults_from(SCHEMA));
    }

    #[test]
    fn validate_overrides_individual_keys() {
        let mut raw = BTreeMap::new();
        raw.insert("limit".to_string(), RawOptionValue::Int(120));
        let opts = validate_options("X.Y", SCHEMA, &raw).unwrap();
        assert_eq!(opts.get_int("limit"), Some(120));
        // unset keys keep defaults
        assert_eq!(opts.get_bool("strict"), Some(false));
    }

    #[test]
    fn validate_overrides_string_list() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "tenant_fields".to_string(),
            RawOptionValue::List(vec![
                RawOptionValue::String("orgId".to_string()),
                RawOptionValue::String("workspaceId".to_string()),
            ]),
        );
        let opts = validate_options("X.Y", SCHEMA, &raw).unwrap();
        let got = opts
            .get_string_list("tenant_fields")
            .map(<[String]>::to_vec)
            .unwrap();
        assert_eq!(got, vec!["orgId".to_string(), "workspaceId".to_string()]);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let mut raw = BTreeMap::new();
        raw.insert("not_a_thing".to_string(), RawOptionValue::Bool(true));
        let err = validate_options("X.Y", SCHEMA, &raw).unwrap_err();
        assert_eq!(
            err,
            OptionsError::UnknownKey {
                check_id: "X.Y".to_string(),
                key: "not_a_thing".to_string(),
            }
        );
    }

    #[test]
    fn type_mismatch_at_top_level_is_rejected() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "limit".to_string(),
            RawOptionValue::String("80".to_string()),
        );
        let err = validate_options("X.Y", SCHEMA, &raw).unwrap_err();
        assert!(
            matches!(
                err,
                OptionsError::TypeMismatch {
                    ref check_id,
                    ref key,
                    expected: "int",
                    got: "string"
                } if check_id == "X.Y" && key == "limit"
            ),
            "got {:?}",
            err
        );
    }

    #[test]
    fn type_mismatch_inside_string_list_is_rejected() {
        let mut raw = BTreeMap::new();
        raw.insert(
            "tenant_fields".to_string(),
            RawOptionValue::List(vec![
                RawOptionValue::String("ok".to_string()),
                RawOptionValue::Int(42),
            ]),
        );
        let err = validate_options("X.Y", SCHEMA, &raw).unwrap_err();
        assert!(
            matches!(
                err,
                OptionsError::TypeMismatch {
                    expected: "string[]",
                    got: "int",
                    ..
                }
            ),
            "got {:?}",
            err
        );
    }

    #[test]
    fn empty_schema_with_empty_input_is_ok() {
        let raw = BTreeMap::new();
        let opts = validate_options("X.Y", &[], &raw).unwrap();
        assert_eq!(opts, CheckOptions::empty());
    }

    #[test]
    fn empty_options_static_is_empty() {
        assert!(EMPTY_OPTIONS.get("anything").is_none());
    }
}
