//! Project-wide architectural spec — `cofferdam.invariants.toml`.
//!
//! One canonical artifact for "what is this codebase supposed to be?" — read
//! by humans, agents, AND multiple checks at once. Promotes the per-check
//! `[layers]` block in `cofferdam.toml` to a shared spec that also covers
//! public-API allowlisting, frozen boundaries, and arbitrary forbid/require
//! import rules.
//!
//! The data types here are dependency-light — a parsed spec is plain `String`
//! / `BTreeMap` data, no globset or glob compilation. Engine + checks compile
//! globs lazily via `crate::layers::build_matchers` (already shared with the
//! cofferdam.toml `[layers]` path) and via `globset` directly inside
//! `Design.BoundaryFrozen` / `Design.InvariantViolation`.
//!
//! ## Schema
//!
//! ```toml
//! [layers]
//! infra  = ["src/infra/**"]
//! domain = ["src/domain/**"]
//! app    = ["src/app/**"]
//!
//! [layers.allow]
//! domain = ["infra"]
//! app    = ["domain", "infra"]
//!
//! [public_api]
//! exports = ["package.json:exports", "src/index.ts"]
//!
//! [boundaries]
//! "src/legacy/**" = { frozen = true, reason = "see ADR-0007" }
//!
//! [invariants]
//! "no-direct-db-access" = { forbid_imports = ["src/infra/db"], from_layers = ["app"] }
//! ```
//!
//! ## Round-trip
//!
//! Round-trip (load → re-serialize → load) is covered by tests in this
//! module — it's the contract `cofferdam advise` and any future
//! `cofferdam invariants normalize` would lean on.
//!
//! ## Schema versioning (cd-9hp.12)
//!
//! Every spec carries a `schema_version` field. The policy is documented
//! in `docs/schema-versioning.md` and summarised here:
//!
//! * `MAJOR.MINOR`, semver-flavoured. Accepted as integer (`1` → `1.0`)
//!   or string (`"1.0"`, `"1.2"`).
//! * `CURRENT_SCHEMA_VERSION` is what this build emits and reads.
//! * `MIN_SUPPORTED_SCHEMA_VERSION` is the oldest version still accepted.
//!   Past versions ≥ `MIN_SUPPORTED` < `CURRENT` are inside the
//!   deprecation window: accepted, but the engine surfaces a one-time
//!   hint telling the user to migrate.
//! * Versions below `MIN_SUPPORTED` are rejected with an actionable
//!   migration message.
//! * Versions above `CURRENT` (any MAJOR) are rejected with an upgrade
//!   message.
//! * A missing `schema_version` is treated as the current MAJOR's `.0`
//!   release for backwards-compatibility with the v0 surface; the
//!   `schema_version_explicit` flag is `false` so the engine can warn.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default filename loaders look for during walk-up discovery.
pub const FILE_NAME: &str = "cofferdam.invariants.toml";

/// Schema version this build of cofferdam emits and reads natively.
///
/// Bump policy: MINOR for additive changes, MAJOR for breaking changes.
/// MAJOR bumps must also extend the deprecation window via
/// `MIN_SUPPORTED_SCHEMA_VERSION` and ship a migration recipe in
/// `docs/schema-versioning.md`.
pub const CURRENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion { major: 1, minor: 0 };

/// Oldest schema version this build still accepts.
///
/// `>= MIN_SUPPORTED` and `< CURRENT` falls in the deprecation window
/// (accepted with a hint). Anything strictly below `MIN_SUPPORTED` is
/// rejected.
pub const MIN_SUPPORTED_SCHEMA_VERSION: SchemaVersion = SchemaVersion { major: 1, minor: 0 };

/// Semver-flavoured `MAJOR.MINOR` version tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
}

impl SchemaVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Parse `"MAJOR"` or `"MAJOR.MINOR"`. Returns the original input
    /// alongside a reason on failure so callers can surface it.
    pub fn parse_str(raw: &str) -> Result<Self, &'static str> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("schema_version string is empty");
        }
        let mut parts = trimmed.split('.');
        let major_str = parts.next().unwrap();
        let major: u32 = major_str
            .parse()
            .map_err(|_| "schema_version MAJOR must be a non-negative integer")?;
        let minor: u32 = match parts.next() {
            None => 0,
            Some(m) => m
                .parse()
                .map_err(|_| "schema_version MINOR must be a non-negative integer")?,
        };
        if parts.next().is_some() {
            return Err("schema_version must be MAJOR or MAJOR.MINOR (no PATCH)");
        }
        Ok(Self { major, minor })
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Outcome of validating a declared schema version against a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionCheck {
    /// Declared version equals `current`. No action needed.
    Ok,
    /// Declared version is older than `current` but ≥ `min_supported`.
    /// Engine should surface a one-time deprecation hint.
    Deprecated,
    /// Declared version exceeds `current`. Reject with upgrade message.
    Future,
    /// Declared version is older than `min_supported`. Reject with
    /// migration message.
    Unsupported,
}

/// Pure validation of a declared schema version against a (current,
/// min_supported) policy. Factored out so tests can exercise the full
/// matrix today, before MAJOR=2 of any schema actually exists.
pub fn validate_version(
    declared: SchemaVersion,
    current: SchemaVersion,
    min_supported: SchemaVersion,
) -> VersionCheck {
    debug_assert!(
        min_supported <= current,
        "policy: min_supported must be ≤ current"
    );
    if declared > current {
        VersionCheck::Future
    } else if declared < min_supported {
        VersionCheck::Unsupported
    } else if declared < current {
        VersionCheck::Deprecated
    } else {
        VersionCheck::Ok
    }
}

/// Parsed spec. Field semantics:
///
/// * `layers` / `layers_allow` mirror the `[layers]` / `[layers.allow]`
///   blocks in `cofferdam.toml`. When both files declare layers, the
///   invariants spec wins (the engine emits a deprecation hint).
/// * `public_api.exports` is a list of "entry-point sentinels" —
///   either a relative path to a TS/JS file, or a `package.json:<key>`
///   pointer. `Design.OrphanExport` skips any export whose file matches
///   one of these entries.
/// * `boundaries[glob].frozen=true` means "no new code in this area";
///   v0 enforcement is glob-match-and-emit (see `Design.BoundaryFrozen`).
///   `reason` is surfaced in the finding message.
/// * `invariants[name]` declares a generic forbid/require import rule.
///   `Design.InvariantViolation` evaluates one finding per violation,
///   keyed by the invariant's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantsSpec {
    /// Schema version this spec declares. Defaults to
    /// `CURRENT_SCHEMA_VERSION` when the field is missing on disk; see
    /// `schema_version_explicit` to tell those cases apart.
    pub schema_version: SchemaVersion,
    /// Whether the spec on disk declared `schema_version` explicitly.
    /// `false` means the loader filled in the default — engine surfaces
    /// a one-time hint encouraging adoption of the explicit form.
    pub schema_version_explicit: bool,
    /// Set when the declared version is older than
    /// `CURRENT_SCHEMA_VERSION` but still within the deprecation
    /// window. Engine surfaces a one-time hint asking the user to
    /// migrate.
    pub schema_version_deprecated: bool,
    /// Absolute path of the directory containing `cofferdam.invariants.toml`.
    /// Glob patterns are matched against paths relative to this root.
    pub project_root: PathBuf,
    /// Layer name → list of glob patterns (gitignore-style).
    pub layers: BTreeMap<String, Vec<String>>,
    /// Layer name → list of layer names whose files this layer is
    /// allowed to import from. An entry of `[]` means "isolated layer".
    pub layers_allow: BTreeMap<String, Vec<String>>,
    /// Public-API entry-point sentinels. Empty when the table is missing.
    pub public_api: PublicApiSpec,
    /// Glob → boundary spec.
    pub boundaries: BTreeMap<String, BoundarySpec>,
    /// Invariant name → spec.
    pub invariants: BTreeMap<String, InvariantSpec>,
}

impl Default for InvariantsSpec {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            schema_version_explicit: false,
            schema_version_deprecated: false,
            project_root: PathBuf::new(),
            layers: BTreeMap::new(),
            layers_allow: BTreeMap::new(),
            public_api: PublicApiSpec::default(),
            boundaries: BTreeMap::new(),
            invariants: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicApiSpec {
    /// Each entry is either a relative file path (`src/index.ts`) or a
    /// `package.json:<key>` pointer (`package.json:exports`). The
    /// `package.json:` form is resolved by the engine at load time; the
    /// resolved set is stored here.
    pub exports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundarySpec {
    pub frozen: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvariantSpec {
    /// Imports matching any of these prefixes (project-relative) are
    /// forbidden when `from_layers` matches (or always, if empty).
    pub forbid_imports: Vec<String>,
    /// Imports matching any of these prefixes are required when
    /// `from_layers` matches. Empty means no requirement.
    pub require_imports: Vec<String>,
    /// Layer-name allowlist. When non-empty the rule applies only to
    /// importing files in those layers; when empty, it applies to all.
    pub from_layers: Vec<String>,
}

/// On-disk shape — what serde reads/writes. The owned `InvariantsSpec`
/// is built from this once project_root is known.
///
/// Field order matters: TOML serialisation places scalars before tables
/// so `schema_version` MUST appear before any `BTreeMap` / table field,
/// otherwise the round-trip output is invalid TOML.
#[derive(Debug, Default, Deserialize, Serialize)]
struct TomlDoc {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    schema_version: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    layers: BTreeMap<String, toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_api: Option<PublicApiToml>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    boundaries: BTreeMap<String, BoundaryToml>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    invariants: BTreeMap<String, InvariantToml>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PublicApiToml {
    #[serde(default)]
    exports: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BoundaryToml {
    #[serde(default)]
    frozen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct InvariantToml {
    #[serde(default)]
    forbid_imports: Vec<String>,
    #[serde(default)]
    require_imports: Vec<String>,
    #[serde(default)]
    from_layers: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum InvariantsError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("{path}: in [layers], `{key}` has unsupported shape ({reason})")]
    BadLayer {
        path: PathBuf,
        key: String,
        reason: &'static str,
    },
    #[error("{path}: schema_version `{raw}` is malformed: {reason}")]
    MalformedSchemaVersion {
        path: PathBuf,
        raw: String,
        reason: &'static str,
    },
    #[error("{path}: schema_version {declared} exceeds this build's maximum supported version ({current}); upgrade cofferdam or pin the spec to a version your build understands")]
    FutureSchemaVersion {
        path: PathBuf,
        declared: SchemaVersion,
        current: SchemaVersion,
    },
    #[error("{path}: schema_version {declared} is no longer supported by this build (minimum supported is {min_supported}); run `cofferdam invariants migrate` against an older cofferdam release or update the spec to a supported version")]
    UnsupportedSchemaVersion {
        path: PathBuf,
        declared: SchemaVersion,
        min_supported: SchemaVersion,
    },
}

impl InvariantsError {
    /// `true` when the error means "the spec is on disk but cannot
    /// safely be loaded". The CLI/engine should fail loudly on these
    /// rather than silently ignore the spec — silently ignoring would
    /// mean the user's architectural rules don't apply without them
    /// knowing.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            InvariantsError::MalformedSchemaVersion { .. }
                | InvariantsError::FutureSchemaVersion { .. }
                | InvariantsError::UnsupportedSchemaVersion { .. }
        )
    }
}

/// Walk up from `start` looking for `cofferdam.invariants.toml`. Stops
/// at the directory containing a `.git` entry. Returns `None` if no
/// spec is found before the repo or filesystem root.
pub fn discover(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}

/// Load and parse a spec from disk.
#[allow(clippy::result_large_err)]
pub fn load(path: &Path) -> Result<InvariantsSpec, InvariantsError> {
    let raw = std::fs::read_to_string(path).map_err(|source| InvariantsError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse(path, &raw)
}

/// Parse a spec from in-memory TOML.
#[allow(clippy::result_large_err)]
pub fn parse(path: &Path, raw: &str) -> Result<InvariantsSpec, InvariantsError> {
    let doc: TomlDoc = toml::from_str(raw).map_err(|source| InvariantsError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let (schema_version, schema_version_explicit) = match doc.schema_version.as_ref() {
        None => (CURRENT_SCHEMA_VERSION, false),
        Some(toml::Value::Integer(i)) if *i >= 0 => (SchemaVersion::new(*i as u32, 0), true),
        Some(toml::Value::Integer(i)) => {
            return Err(InvariantsError::MalformedSchemaVersion {
                path: path.to_path_buf(),
                raw: i.to_string(),
                reason: "schema_version integer must be non-negative",
            });
        }
        Some(toml::Value::String(s)) => match SchemaVersion::parse_str(s) {
            Ok(v) => (v, true),
            Err(reason) => {
                return Err(InvariantsError::MalformedSchemaVersion {
                    path: path.to_path_buf(),
                    raw: s.clone(),
                    reason,
                });
            }
        },
        Some(other) => {
            return Err(InvariantsError::MalformedSchemaVersion {
                path: path.to_path_buf(),
                raw: other.to_string(),
                reason: "schema_version must be an integer or `MAJOR.MINOR` string",
            });
        }
    };

    let schema_version_deprecated = match validate_version(
        schema_version,
        CURRENT_SCHEMA_VERSION,
        MIN_SUPPORTED_SCHEMA_VERSION,
    ) {
        VersionCheck::Ok => false,
        VersionCheck::Deprecated => true,
        VersionCheck::Future => {
            return Err(InvariantsError::FutureSchemaVersion {
                path: path.to_path_buf(),
                declared: schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        VersionCheck::Unsupported => {
            return Err(InvariantsError::UnsupportedSchemaVersion {
                path: path.to_path_buf(),
                declared: schema_version,
                min_supported: MIN_SUPPORTED_SCHEMA_VERSION,
            });
        }
    };

    let mut layers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut layers_allow: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, val) in doc.layers {
        if key == "allow" {
            let table = match val {
                toml::Value::Table(t) => t,
                _ => {
                    return Err(InvariantsError::BadLayer {
                        path: path.to_path_buf(),
                        key,
                        reason: "expected a table mapping layer-name → array of layer names",
                    });
                }
            };
            for (layer_name, deps) in table {
                let arr = match deps {
                    toml::Value::Array(a) => a,
                    _ => {
                        return Err(InvariantsError::BadLayer {
                            path: path.to_path_buf(),
                            key: format!("allow.{}", layer_name),
                            reason: "expected an array of layer-name strings",
                        });
                    }
                };
                let mut names = Vec::with_capacity(arr.len());
                for item in arr {
                    match item {
                        toml::Value::String(s) => names.push(s),
                        _ => {
                            return Err(InvariantsError::BadLayer {
                                path: path.to_path_buf(),
                                key: format!("allow.{}", layer_name),
                                reason: "expected a string layer-name",
                            });
                        }
                    }
                }
                layers_allow.insert(layer_name, names);
            }
            continue;
        }

        let arr = match val {
            toml::Value::Array(a) => a,
            _ => {
                return Err(InvariantsError::BadLayer {
                    path: path.to_path_buf(),
                    key,
                    reason: "expected an array of glob strings",
                });
            }
        };
        let mut globs = Vec::with_capacity(arr.len());
        for item in arr {
            match item {
                toml::Value::String(s) => globs.push(s),
                _ => {
                    return Err(InvariantsError::BadLayer {
                        path: path.to_path_buf(),
                        key,
                        reason: "expected a string glob",
                    });
                }
            }
        }
        layers.insert(key, globs);
    }

    let public_api = doc
        .public_api
        .map(|p| PublicApiSpec { exports: p.exports })
        .unwrap_or_default();

    let boundaries: BTreeMap<String, BoundarySpec> = doc
        .boundaries
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                BoundarySpec {
                    frozen: v.frozen,
                    reason: v.reason,
                },
            )
        })
        .collect();

    let invariants: BTreeMap<String, InvariantSpec> = doc
        .invariants
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                InvariantSpec {
                    forbid_imports: v.forbid_imports,
                    require_imports: v.require_imports,
                    from_layers: v.from_layers,
                },
            )
        })
        .collect();

    let project_root = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    Ok(InvariantsSpec {
        schema_version,
        schema_version_explicit,
        schema_version_deprecated,
        project_root,
        layers,
        layers_allow,
        public_api,
        boundaries,
        invariants,
    })
}

/// Re-serialize a parsed spec back to TOML — useful for round-trip tests
/// and for tools that normalize an existing spec on disk.
pub fn to_toml_string(spec: &InvariantsSpec) -> Result<String, toml::ser::Error> {
    let mut layers_doc: BTreeMap<String, toml::Value> = BTreeMap::new();
    for (name, globs) in &spec.layers {
        layers_doc.insert(
            name.clone(),
            toml::Value::Array(globs.iter().cloned().map(toml::Value::String).collect()),
        );
    }
    if !spec.layers_allow.is_empty() {
        let mut allow_table = toml::map::Map::new();
        for (name, deps) in &spec.layers_allow {
            allow_table.insert(
                name.clone(),
                toml::Value::Array(deps.iter().cloned().map(toml::Value::String).collect()),
            );
        }
        layers_doc.insert("allow".to_string(), toml::Value::Table(allow_table));
    }

    let public_api = if spec.public_api.exports.is_empty() {
        None
    } else {
        Some(PublicApiToml {
            exports: spec.public_api.exports.clone(),
        })
    };

    let boundaries: BTreeMap<String, BoundaryToml> = spec
        .boundaries
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                BoundaryToml {
                    frozen: v.frozen,
                    reason: v.reason.clone(),
                },
            )
        })
        .collect();

    let invariants: BTreeMap<String, InvariantToml> = spec
        .invariants
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                InvariantToml {
                    forbid_imports: v.forbid_imports.clone(),
                    require_imports: v.require_imports.clone(),
                    from_layers: v.from_layers.clone(),
                },
            )
        })
        .collect();

    let doc = TomlDoc {
        schema_version: Some(toml::Value::String(spec.schema_version.to_string())),
        layers: layers_doc,
        public_api,
        boundaries,
        invariants,
    };
    toml::to_string_pretty(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const FULL_SPEC: &str = r#"
schema_version = "1.0"

[layers]
infra  = ["src/infra/**"]
domain = ["src/domain/**"]
app    = ["src/app/**"]

[layers.allow]
domain = ["infra"]
app    = ["domain", "infra"]

[public_api]
exports = ["package.json:exports", "src/index.ts"]

[boundaries]
"src/legacy/**" = { frozen = true, reason = "see ADR-0007" }

[invariants]
"no-direct-db-access" = { forbid_imports = ["src/infra/db"], from_layers = ["app"] }
"#;

    #[test]
    fn parses_full_spec() {
        let spec = parse(Path::new("cofferdam.invariants.toml"), FULL_SPEC).expect("parse");
        assert_eq!(spec.schema_version, SchemaVersion::new(1, 0));
        assert!(spec.schema_version_explicit);
        assert!(!spec.schema_version_deprecated);
        assert_eq!(spec.layers.len(), 3);
        assert_eq!(spec.layers_allow.get("app").unwrap(), &["domain", "infra"]);
        assert_eq!(
            spec.public_api.exports,
            vec![
                "package.json:exports".to_string(),
                "src/index.ts".to_string()
            ]
        );
        let legacy = spec.boundaries.get("src/legacy/**").expect("present");
        assert!(legacy.frozen);
        assert_eq!(legacy.reason.as_deref(), Some("see ADR-0007"));
        let inv = spec.invariants.get("no-direct-db-access").expect("present");
        assert_eq!(inv.forbid_imports, vec!["src/infra/db".to_string()]);
        assert_eq!(inv.from_layers, vec!["app".to_string()]);
    }

    #[test]
    fn empty_spec_yields_default() {
        let spec = parse(Path::new("test.toml"), "").expect("parse");
        assert!(spec.layers.is_empty());
        assert!(spec.boundaries.is_empty());
        assert!(spec.invariants.is_empty());
        assert!(spec.public_api.exports.is_empty());
    }

    #[test]
    fn round_trip_preserves_data() {
        let path = Path::new("cofferdam.invariants.toml");
        let parsed = parse(path, FULL_SPEC).expect("parse");
        let serialized = to_toml_string(&parsed).expect("serialize");
        let reparsed = parse(path, &serialized).expect("reparse");
        // project_root differs because parse fills it from path.parent();
        // strip it for the equality check.
        let mut a = parsed.clone();
        let mut b = reparsed;
        a.project_root.clear();
        b.project_root.clear();
        assert_eq!(a, b);
    }

    #[test]
    fn discover_walks_up() {
        let dir = tempdir().expect("tempdir");
        let nested = dir.path().join("a/b");
        fs::create_dir_all(&nested).expect("mkdir");
        let cfg = dir.path().join(FILE_NAME);
        fs::write(&cfg, FULL_SPEC).expect("write");
        let found = discover(&nested).expect("found");
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(cfg).unwrap()
        );
    }

    #[test]
    fn discover_stops_at_git_root() {
        let dir = tempdir().expect("tempdir");
        let outer = dir.path().join(FILE_NAME);
        fs::write(&outer, "").expect("write outer");
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).expect(".git");
        let nested = repo.join("src");
        fs::create_dir_all(&nested).expect("nested");
        assert_eq!(discover(&nested), None);
    }

    #[test]
    fn bad_layers_shape_errors() {
        let raw = r#"
[layers]
broken = "should be an array"
"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, InvariantsError::BadLayer { .. }));
    }

    #[test]
    fn bad_layer_allow_shape_errors() {
        let raw = r#"
[layers.allow]
app = "should be an array"
"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, InvariantsError::BadLayer { .. }));
    }

    // ----- schema_version (cd-9hp.12) -----

    #[test]
    fn schema_version_missing_defaults_to_current_implicit() {
        let raw = r#"
[layers]
app = ["src/app/**"]
"#;
        let spec = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(spec.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(
            !spec.schema_version_explicit,
            "missing field should be marked implicit"
        );
        assert!(!spec.schema_version_deprecated);
    }

    #[test]
    fn schema_version_accepts_integer_form() {
        let raw = r#"
schema_version = 1

[layers]
app = ["src/app/**"]
"#;
        let spec = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(spec.schema_version, SchemaVersion::new(1, 0));
        assert!(spec.schema_version_explicit);
    }

    #[test]
    fn schema_version_accepts_string_form() {
        let raw = r#"schema_version = "1.0""#;
        let spec = parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(spec.schema_version, SchemaVersion::new(1, 0));
        assert!(spec.schema_version_explicit);
    }

    #[test]
    fn schema_version_rejects_future_major() {
        // CURRENT is 1.0 today; any 2.x must be rejected.
        let raw = r#"schema_version = "2.0""#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, InvariantsError::FutureSchemaVersion { .. }));
        assert!(err.is_fatal());
    }

    #[test]
    fn schema_version_rejects_future_minor() {
        // 1.5 today is unknown to this build (CURRENT is 1.0).
        let raw = r#"schema_version = "1.5""#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, InvariantsError::FutureSchemaVersion { .. }));
    }

    #[test]
    fn schema_version_rejects_malformed_string() {
        let raw = r#"schema_version = "1.0.0""#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(
            err,
            InvariantsError::MalformedSchemaVersion { .. }
        ));
    }

    #[test]
    fn schema_version_rejects_negative_integer() {
        let raw = r#"schema_version = -1"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(
            err,
            InvariantsError::MalformedSchemaVersion { .. }
        ));
    }

    #[test]
    fn schema_version_rejects_wrong_type() {
        let raw = r#"schema_version = true"#;
        let err = parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(
            err,
            InvariantsError::MalformedSchemaVersion { .. }
        ));
    }

    // The policy logic is exercised against synthetic (current,
    // min_supported) values so we cover all four `VersionCheck` arms
    // today, before any real schema MAJOR=2 exists.

    #[test]
    fn validate_version_ok() {
        let r = validate_version(
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
        );
        assert_eq!(r, VersionCheck::Ok);
    }

    #[test]
    fn validate_version_in_deprecation_window() {
        // Hypothetical future state: current=2.0, min=1.0. A declared
        // 1.5 is older than current but ≥ min — accepted with hint.
        let r = validate_version(
            SchemaVersion::new(1, 5),
            SchemaVersion::new(2, 0),
            SchemaVersion::new(1, 0),
        );
        assert_eq!(r, VersionCheck::Deprecated);
    }

    #[test]
    fn validate_version_future_rejected() {
        let r = validate_version(
            SchemaVersion::new(2, 0),
            SchemaVersion::new(1, 0),
            SchemaVersion::new(1, 0),
        );
        assert_eq!(r, VersionCheck::Future);
    }

    #[test]
    fn validate_version_out_of_window_rejected() {
        // Hypothetical: current=3.0, min=2.0. A declared 1.0 is below
        // the deprecation window — rejected with migration message.
        let r = validate_version(
            SchemaVersion::new(1, 0),
            SchemaVersion::new(3, 0),
            SchemaVersion::new(2, 0),
        );
        assert_eq!(r, VersionCheck::Unsupported);
    }

    #[test]
    fn schema_version_parse_str_variants() {
        assert_eq!(SchemaVersion::parse_str("1"), Ok(SchemaVersion::new(1, 0)));
        assert_eq!(
            SchemaVersion::parse_str("2.3"),
            Ok(SchemaVersion::new(2, 3))
        );
        assert!(SchemaVersion::parse_str("").is_err());
        assert!(SchemaVersion::parse_str("1.2.3").is_err());
        assert!(SchemaVersion::parse_str("a.b").is_err());
    }

    #[test]
    fn round_trip_canonicalises_version_to_string() {
        // Input declares as integer; output should declare as "1.0".
        let raw = r#"
schema_version = 1

[layers]
app = ["src/app/**"]
"#;
        let parsed = parse(Path::new("test.toml"), raw).expect("parse");
        let serialized = to_toml_string(&parsed).expect("serialize");
        assert!(
            serialized.contains(r#"schema_version = "1.0""#),
            "serializer should emit canonical MAJOR.MINOR string, got: {serialized}"
        );
        // And the canonical form re-parses to the same version.
        let reparsed = parse(Path::new("test.toml"), &serialized).expect("reparse");
        assert_eq!(reparsed.schema_version, SchemaVersion::new(1, 0));
        assert!(reparsed.schema_version_explicit);
    }
}
