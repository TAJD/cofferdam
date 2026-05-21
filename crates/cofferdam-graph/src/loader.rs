//! Load-time schema versioning for serialised canonical-graph
//! bundles (cd-9hp.9 cp5).
//!
//! The canonical graph lives in-memory in
//! [`crate::corpus_slot::CANONICAL_GRAPH`] today. No adapter or
//! incremental-analysis pipeline emits a serialised graph yet —
//! those formats are owned by cd-9hp.10 (adapter contract) and
//! cd-9hp.4 (incremental cache). This module ships the **version
//! envelope** they'll inherit: a JSON object with a reserved
//! `schema_version` key, validated against the same
//! `MAJOR.MINOR` policy that `cofferdam.invariants.toml` already
//! enforces (cd-9hp.12).
//!
//! Wiring the policy now, before any consumer ships, follows the
//! pattern cd-9hp.12 established: the full `VersionCheck` matrix is
//! exercised against hypothetical states so the deprecation-window
//! logic is testable today, before MAJOR=2 of the graph schema
//! actually exists.
//!
//! ## Envelope shape
//!
//! ```json
//! {
//!   "schema_version": "1.0",
//!   "body": { ... adapter / cache payload ... }
//! }
//! ```
//!
//! The top-level `schema_version` is the only key this module reads
//! and validates. Every other key is captured into
//! [`GraphEnvelope::body`] for the consumer (cd-9hp.10, cd-9hp.4) to
//! interpret. The split keeps cp5's commitment minimal — we lock in
//! the version-checking contract without committing to a payload
//! schema before its consumers exist.

use std::path::PathBuf;

use cofferdam_core::invariants::{validate_version, SchemaVersion, VersionCheck};
use serde_json::{Map, Value};

use crate::{CURRENT_SCHEMA_VERSION, MIN_SUPPORTED_SCHEMA_VERSION};

/// A loaded canonical-graph envelope. The version has already been
/// validated against `(CURRENT, MIN_SUPPORTED)`; `body` is the
/// remaining top-level JSON keys, unfiltered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEnvelope {
    /// Schema version this bundle declares. Always within the
    /// `[MIN_SUPPORTED, CURRENT]` window — versions outside it are
    /// rejected by [`load_envelope`].
    pub schema_version: SchemaVersion,
    /// `true` when the declared version is older than
    /// [`CURRENT_SCHEMA_VERSION`] but still inside the deprecation
    /// window. The engine surfaces a one-time migration hint when set.
    pub schema_version_deprecated: bool,
    /// Top-level JSON keys other than `schema_version`. Format is
    /// owned by the consumer (cd-9hp.10 / cd-9hp.4).
    pub body: Map<String, Value>,
}

/// Errors that prevent a canonical-graph envelope from loading.
///
/// All variants are fatal — silently accepting a graph from a future
/// or unsupported schema would let the engine re-interpret rules in
/// ways the producer never consented to. Same loudness contract as
/// `InvariantsError` in `cofferdam-core`.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to parse canonical-graph envelope{path_hint}: {source}", path_hint = format_path_hint(.path))]
    Parse {
        path: Option<PathBuf>,
        #[source]
        source: serde_json::Error,
    },
    #[error("canonical-graph envelope{path_hint} must be a JSON object at the top level", path_hint = format_path_hint(.path))]
    NotAnObject { path: Option<PathBuf> },
    #[error("canonical-graph envelope{path_hint} is missing the `schema_version` field; cofferdam {current} reads MAJOR.MINOR versions {min_supported}..={current}", path_hint = format_path_hint(.path))]
    MissingSchemaVersion {
        path: Option<PathBuf>,
        current: SchemaVersion,
        min_supported: SchemaVersion,
    },
    #[error("canonical-graph envelope{path_hint}: schema_version `{raw}` is malformed: {reason}", path_hint = format_path_hint(.path))]
    MalformedSchemaVersion {
        path: Option<PathBuf>,
        raw: String,
        reason: &'static str,
    },
    #[error("canonical-graph envelope{path_hint}: schema_version {declared} exceeds this build's maximum supported version ({current}); upgrade cofferdam (this build is {cofferdam_version}) or re-emit the graph against a version this build understands", path_hint = format_path_hint(.path))]
    FutureSchemaVersion {
        path: Option<PathBuf>,
        declared: SchemaVersion,
        current: SchemaVersion,
        cofferdam_version: &'static str,
    },
    #[error("canonical-graph envelope{path_hint}: schema_version {declared} is no longer supported by this build (minimum supported is {min_supported}); regenerate the graph with a current cofferdam build", path_hint = format_path_hint(.path))]
    UnsupportedSchemaVersion {
        path: Option<PathBuf>,
        declared: SchemaVersion,
        min_supported: SchemaVersion,
    },
}

fn format_path_hint(path: &Option<PathBuf>) -> String {
    match path {
        Some(p) => format!(" `{}`", p.display()),
        None => String::new(),
    }
}

impl LoadError {
    /// Every load error is fatal — silently accepting an envelope
    /// from an unknown schema would defeat the purpose of versioning.
    /// Surface kept for parity with `InvariantsError::is_fatal`.
    pub fn is_fatal(&self) -> bool {
        true
    }
}

/// Validate a declared schema version against this build's graph
/// policy. Pure wrapper around
/// [`cofferdam_core::invariants::validate_version`] that maps the
/// `VersionCheck` outcome to the graph-side error taxonomy.
///
/// Returns `Ok(deprecated)` when the version is inside the window —
/// `deprecated == true` when the caller should surface a migration
/// hint.
pub fn validate_envelope_version(declared: SchemaVersion) -> Result<bool, LoadError> {
    validate_envelope_version_against(
        declared,
        CURRENT_SCHEMA_VERSION,
        MIN_SUPPORTED_SCHEMA_VERSION,
        None,
    )
}

/// Like [`validate_envelope_version`] but takes the policy
/// `(current, min_supported)` and an optional path explicitly.
///
/// Factored out so tests can exercise the full `VersionCheck` matrix
/// against hypothetical policy states today, before MAJOR=2 of the
/// graph schema exists. Production callers should use
/// [`validate_envelope_version`] (which pins this build's policy).
pub fn validate_envelope_version_against(
    declared: SchemaVersion,
    current: SchemaVersion,
    min_supported: SchemaVersion,
    path: Option<PathBuf>,
) -> Result<bool, LoadError> {
    match validate_version(declared, current, min_supported) {
        VersionCheck::Ok => Ok(false),
        VersionCheck::Deprecated => Ok(true),
        VersionCheck::Future => Err(LoadError::FutureSchemaVersion {
            path,
            declared,
            current,
            cofferdam_version: env!("CARGO_PKG_VERSION"),
        }),
        VersionCheck::Unsupported => Err(LoadError::UnsupportedSchemaVersion {
            path,
            declared,
            min_supported,
        }),
    }
}

/// Parse a `schema_version` value (integer or `MAJOR.MINOR` string)
/// from a raw JSON value. Mirrors the TOML parsing in
/// `cofferdam_core::invariants::parse` so both surfaces accept the
/// same forms.
fn extract_schema_version(raw: &Value, path: &Option<PathBuf>) -> Result<SchemaVersion, LoadError> {
    match raw {
        Value::Number(n) => {
            // JSON has no integer/float split; accept exact non-negative
            // integers, reject everything else.
            if let Some(i) = n.as_u64() {
                if i > u32::MAX as u64 {
                    return Err(LoadError::MalformedSchemaVersion {
                        path: path.clone(),
                        raw: n.to_string(),
                        reason: "schema_version MAJOR exceeds u32",
                    });
                }
                Ok(SchemaVersion::new(i as u32, 0))
            } else {
                Err(LoadError::MalformedSchemaVersion {
                    path: path.clone(),
                    raw: n.to_string(),
                    reason: "schema_version integer must be a non-negative whole number",
                })
            }
        }
        Value::String(s) => {
            SchemaVersion::parse_str(s).map_err(|reason| LoadError::MalformedSchemaVersion {
                path: path.clone(),
                raw: s.clone(),
                reason,
            })
        }
        other => Err(LoadError::MalformedSchemaVersion {
            path: path.clone(),
            raw: other.to_string(),
            reason: "schema_version must be an integer or `MAJOR.MINOR` string",
        }),
    }
}

/// Parse a canonical-graph envelope from JSON bytes, validating
/// `schema_version` against this build's policy.
///
/// `path` is optional — supply it when loading from disk so error
/// messages can point the user at the offending file. Pass `None`
/// for in-memory loads (tests, network-delivered bundles).
pub fn load_envelope(bytes: &[u8], path: Option<PathBuf>) -> Result<GraphEnvelope, LoadError> {
    let raw: Value = serde_json::from_slice(bytes).map_err(|source| LoadError::Parse {
        path: path.clone(),
        source,
    })?;
    let Value::Object(mut obj) = raw else {
        return Err(LoadError::NotAnObject { path });
    };
    let Some(version_raw) = obj.remove("schema_version") else {
        return Err(LoadError::MissingSchemaVersion {
            path,
            current: CURRENT_SCHEMA_VERSION,
            min_supported: MIN_SUPPORTED_SCHEMA_VERSION,
        });
    };
    let schema_version = extract_schema_version(&version_raw, &path)?;
    let schema_version_deprecated = validate_envelope_version_against(
        schema_version,
        CURRENT_SCHEMA_VERSION,
        MIN_SUPPORTED_SCHEMA_VERSION,
        path,
    )?;
    Ok(GraphEnvelope {
        schema_version,
        schema_version_deprecated,
        body: obj,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- envelope happy paths -----

    #[test]
    fn accepts_current_version_string() {
        let bytes = br#"{ "schema_version": "1.0", "body": { "nodes": [] } }"#;
        let env = load_envelope(bytes, None).expect("load");
        assert_eq!(env.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(!env.schema_version_deprecated);
        // body retains the non-version keys verbatim.
        assert!(env.body.contains_key("body"));
        assert!(!env.body.contains_key("schema_version"));
    }

    #[test]
    fn accepts_current_version_integer() {
        let bytes = br#"{ "schema_version": 1 }"#;
        let env = load_envelope(bytes, None).expect("load");
        assert_eq!(env.schema_version, SchemaVersion::new(1, 0));
    }

    // ----- envelope rejection paths -----

    #[test]
    fn rejects_non_object_root() {
        let err = load_envelope(b"[1, 2, 3]", None).unwrap_err();
        assert!(matches!(err, LoadError::NotAnObject { .. }));
        assert!(err.is_fatal());
    }

    #[test]
    fn rejects_malformed_json() {
        let err = load_envelope(b"{ not json", None).unwrap_err();
        assert!(matches!(err, LoadError::Parse { .. }));
    }

    #[test]
    fn rejects_missing_version() {
        let bytes = br#"{ "body": {} }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::MissingSchemaVersion { .. }));
        // The error names the supported window so the user knows what
        // to declare.
        let rendered = err.to_string();
        assert!(rendered.contains("schema_version"));
        assert!(
            rendered.contains(&CURRENT_SCHEMA_VERSION.to_string()),
            "error must surface CURRENT version, got: {rendered}"
        );
    }

    #[test]
    fn rejects_malformed_version_string() {
        let bytes = br#"{ "schema_version": "1.0.0" }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::MalformedSchemaVersion { .. }));
    }

    #[test]
    fn rejects_wrong_type_for_version() {
        let bytes = br#"{ "schema_version": true }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::MalformedSchemaVersion { .. }));
    }

    #[test]
    fn rejects_negative_integer_version() {
        let bytes = br#"{ "schema_version": -1 }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::MalformedSchemaVersion { .. }));
    }

    #[test]
    fn rejects_future_major() {
        // CURRENT is 1.0 today; any 2.x must be rejected with an
        // upgrade message that names this cofferdam build's version.
        let bytes = br#"{ "schema_version": "2.0" }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::FutureSchemaVersion { .. }));
        let rendered = err.to_string();
        assert!(
            rendered.contains(env!("CARGO_PKG_VERSION")),
            "future-version error must name the cofferdam build, got: {rendered}"
        );
    }

    #[test]
    fn rejects_future_minor() {
        let bytes = br#"{ "schema_version": "1.5" }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        assert!(matches!(err, LoadError::FutureSchemaVersion { .. }));
    }

    // ----- pure policy: exercises all four VersionCheck arms -----
    //
    // MIN_SUPPORTED == CURRENT == 1.0 today, so Deprecated and
    // Unsupported can't be reached against the production policy.
    // These tests feed synthetic (current, min_supported) values to
    // `validate_envelope_version_against` so the deprecation-window
    // wiring is covered before MAJOR=2 of the graph schema exists.

    #[test]
    fn policy_ok_returns_not_deprecated() {
        let r = validate_envelope_version_against(
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
            None,
        );
        assert!(!r.unwrap());
    }

    #[test]
    fn policy_in_deprecation_window_returns_deprecated_true() {
        // Hypothetical future state: current=2.0, min=1.0. A declared
        // 1.5 is older than current but ≥ min — accepted with hint.
        let r = validate_envelope_version_against(
            SchemaVersion::new(1, 5),
            SchemaVersion::new(2, 0),
            SchemaVersion::new(1, 0),
            None,
        );
        assert!(r.unwrap());
    }

    #[test]
    fn policy_future_rejected_with_actionable_message() {
        let r = validate_envelope_version_against(
            SchemaVersion::new(2, 0),
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
            Some(PathBuf::from("/tmp/x.graph.json")),
        );
        let err = r.unwrap_err();
        assert!(matches!(err, LoadError::FutureSchemaVersion { .. }));
        let rendered = err.to_string();
        // Path is surfaced so the user knows which file failed.
        assert!(
            rendered.contains("x.graph.json"),
            "error must name the offending path, got: {rendered}"
        );
        // Both versions are named.
        assert!(rendered.contains("2.0"));
        assert!(rendered.contains("1.0"));
    }

    #[test]
    fn policy_out_of_window_rejected_with_migration_message() {
        // Hypothetical: current=3.0, min=2.0. A declared 1.0 is below
        // the deprecation window.
        let r = validate_envelope_version_against(
            SchemaVersion::new(1, 0),
            SchemaVersion::new(3, 0),
            SchemaVersion::new(2, 0),
            None,
        );
        let err = r.unwrap_err();
        assert!(matches!(err, LoadError::UnsupportedSchemaVersion { .. }));
        let rendered = err.to_string();
        assert!(rendered.contains("no longer supported"));
        assert!(rendered.contains("1.0"));
        assert!(rendered.contains("2.0"));
    }

    // ----- path hints surface in messages -----

    #[test]
    fn path_hint_appears_in_message_when_set() {
        let bytes = br#"{ "schema_version": "9.9" }"#;
        let err = load_envelope(bytes, Some(PathBuf::from("graphs/cache.json"))).unwrap_err();
        assert!(err.to_string().contains("graphs/cache.json"));
    }

    #[test]
    fn path_hint_absent_when_none() {
        let bytes = br#"{ "schema_version": "9.9" }"#;
        let err = load_envelope(bytes, None).unwrap_err();
        // No leading backtick segment when path is None.
        let rendered = err.to_string();
        assert!(
            !rendered.contains("envelope `"),
            "no path → no backtick-quoted hint, got: {rendered}"
        );
    }
}
