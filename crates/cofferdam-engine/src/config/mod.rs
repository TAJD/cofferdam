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

pub mod loader;
pub mod options;
pub mod resolution;
pub mod schema;

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use cofferdam_core::graph::LayersConfig;
use cofferdam_core::invariants::InvariantsError;
use cofferdam_core::OptionsError;

// Re-export all public types and functions from submodules
pub use loader::{discover, load, path_key, FILE_NAME};
pub use options::{options_for, options_for_raw, unknown_check_ids};
pub use resolution::{
    resolve_for_targets, resolve_with_invariants, target_anchor, LoadDiagnostics,
};
pub use schema::{unknown_keys, KeySpec, Keys, SectionSpec, UnknownKey, SECTIONS};

/// Parsed project config: per-check raw option bags + per-check
/// severity overrides. Unknown check IDs are stored verbatim and
/// surfaced via `unknown_check_ids` so the CLI can warn without
/// failing the build over a typo.
#[derive(Debug, Clone, Default)]
pub struct ProjectConfig {
    pub checks: BTreeMap<String, BTreeMap<String, cofferdam_core::RawOptionValue>>,
    /// Per-check severity overrides parsed from `[checks."X.Y"] severity = "..."`.
    /// Keyed by check_id. Engine consults this in its severity post-pass.
    pub severity_overrides: BTreeMap<String, cofferdam_core::Severity>,
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
    pub invariants: Option<cofferdam_core::invariants::InvariantsSpec>,
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
    /// `[engine] extra_extensions` — extra file extensions (without
    /// leading dot) discovery should walk beyond `DEFAULT_EXTENSIONS`
    /// (CD-68). Empty when not set. The CLI merges these into
    /// `DiscoveryOptions::extensions` before walking.
    pub engine_extra_extensions: Vec<String>,
    /// `[[overrides]]` blocks — per-path-glob check config (cd-m5tu).
    /// Each block scopes `limit`/`severity`/`disabled` (and any other
    /// per-check option) to files matching its `paths` globs, while
    /// every other check keeps running on those files. Empty when none
    /// are declared. The engine applies them per-file in declaration
    /// order; the last matching block wins per (check, key).
    pub overrides: Vec<OverrideBlock>,
    /// `[budgets]` table (CD-64 D2) — caps on the number of findings
    /// allowed for a check id (e.g. `"Refactor.CognitiveComplexity"`) or
    /// a whole category (e.g. `"Refactor"`). Counts include baselined
    /// findings, so a budget catches debt a severity-based `--fail-on`
    /// gate would otherwise let through un-gated. Enforced by `cofferdam
    /// check`; `cofferdam baseline ratchet` lowers (never raises) these
    /// values to match the current finding count.
    pub budgets: BTreeMap<String, u32>,
    /// `[[context_suppress]]` blocks (CD-212) — per-`check_id` path-glob
    /// suppression for `cofferdam context` digest items. Unlike
    /// `[[overrides]]`, these don't scope check *options*; they drop
    /// matching `ContextItem`s from the digest entirely, advisory-only
    /// (never fails `cofferdam context`, which always exits 0 outside
    /// usage errors). Empty when none are declared.
    pub context_suppress: Vec<ContextSuppressRule>,
    /// Keys the file declares that no section of the schema recognises
    /// (CD-311). Serde skips them, so without this they would be
    /// silently inert — a rule that never fires and a green run. Surfaced
    /// through `LoadDiagnostics` as warnings, the same way an unknown
    /// check id is: a typo should be visible, not fatal.
    pub unknown_keys: Vec<schema::UnknownKey>,
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
    pub options: BTreeMap<String, cofferdam_core::RawOptionValue>,
    /// `severity = "..."` for this check on matching files. `None`
    /// leaves the global severity in place.
    pub severity: Option<cofferdam_core::Severity>,
    /// `disabled = true` skips the check entirely for matching files;
    /// `disabled = false` re-enables it (so a later block can undo an
    /// earlier one). `None` leaves the prior decision untouched.
    pub disabled: Option<bool>,
}

/// Reduce an absolute, engine-promoted `file_key` to the
/// project-relative form globs are written against: strip the absolute
/// `root_key` prefix. Returns `None` when `file_key` isn't under
/// `root_key` at all — callers must treat that as "does not match"
/// rather than falling back to matching the raw (still-absolute) path,
/// since a `**/…` pattern would otherwise match straight through an
/// absolute-path prefix it was never meant to see (CD-226). Shared by
/// [`OverrideBlock::is_match`] and [`ContextSuppressRule::is_match`].
/// CD-225: the naive `file_key.strip_prefix(root_key)` this used to fall
/// back to (without requiring a separator after the match) mis-strips a
/// sibling directory whose name merely *extends* `root_key`'s — e.g.
/// `root_key = "/repo"` against `file_key = "/repo-backup/a.ts"` yielded
/// `"-backup/a.ts"`, a path that was never inside the project root but
/// now reads as project-relative to the globset. The only case that
/// fallback needs beyond the separator-anchored `with_slash` match above
/// is `file_key == root_key` exactly (a directory, not a file, so no
/// real caller passes it) — so it's handled explicitly instead. A
/// leading `./` on `file_key` (a relative, non-engine-promoted path) is
/// always project-relative already and is returned as-is.
///
/// CD-232: the out-of-root rejection must recognise every absolute or
/// escaping form `file_key` can take, not just a leading `/`. `path_key`
/// (loader.rs) normalises backslashes to `/` before either key reaches
/// here, so a bare `\`-prefix check is dead code on Windows — but a
/// Windows key still carries a `c:/`-style drive prefix (lowercased by
/// `path_key`), which a leading-`/`-only check lets straight through as
/// "already relative", reopening the CD-226 sibling-directory leak on
/// Windows. A `../`-prefixed key (escaping the root upward) needs the
/// same rejection for the same reason.
fn relativize<'a>(root_key: &str, file_key: &'a str) -> Option<&'a str> {
    if let Some(stripped) = file_key.strip_prefix("./") {
        return Some(stripped);
    }
    // CD-231: trim any trailing slash on `root_key` before deriving
    // both the prefix-with-slash and the exact-match comparison, so a
    // caller-supplied `root_key` of "/repo/" behaves identically to
    // "/repo" — without this, `file_key == root_key` never matched the
    // trailing-slash form (it compared "/repo" against "/repo/"),
    // silently falling through to `None` instead of `Some("")`.
    let root_trimmed = root_key.trim_end_matches('/');
    let with_slash = format!("{root_trimmed}/");
    if let Some(stripped) = file_key.strip_prefix(with_slash.as_str()) {
        Some(stripped)
    } else if file_key == root_trimmed {
        Some("")
    } else if is_absolute_or_escaping(file_key) {
        None
    } else {
        Some(file_key)
    }
}

/// True when `key` is unambiguously not root-relative: a leading `/`
/// (POSIX absolute), a drive-letter prefix (`c:/...`, Windows
/// absolute), or a `../` escape upward out of the root. See
/// [`relativize`]'s CD-232 doc note — anything not covered by this
/// check falls through to being treated as already project-relative.
fn is_absolute_or_escaping(key: &str) -> bool {
    if key.starts_with('/') || key == ".." || key.starts_with("../") {
        return true;
    }
    let bytes = key.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

impl OverrideBlock {
    /// Test whether `file_key` (a forward-slash, normalised path — may
    /// be absolute or relative) is matched by this block's globs.
    /// Mirrors `public_api`'s matcher: strip the absolute root prefix
    /// (or a leading `./`) so root-relative patterns match an absolute
    /// engine-promoted path. A `file_key` outside `root_key` never
    /// matches (CD-226) — it is not relativized and handed to the
    /// globset as-is.
    pub fn is_match(&self, file_key: &str) -> bool {
        match relativize(&self.root_key, file_key) {
            Some(relative) => self.globset.is_match(relative),
            None => false,
        }
    }
}

/// One `[[context_suppress]]` block (CD-212): drop `cofferdam context`
/// digest items for `check_id` whose anchor file(s) match `paths`.
#[derive(Debug, Clone)]
pub struct ContextSuppressRule {
    /// The `Context.*` provider this rule applies to (e.g.
    /// `"Context.Precedent"`).
    pub check_id: String,
    /// Raw glob patterns as written, kept for diagnostics and hashing.
    pub paths: Vec<String>,
    /// Compiled matcher over `paths`.
    pub globset: globset::GlobSet,
    /// Normalised absolute root prefix, same convention as
    /// [`OverrideBlock::root_key`].
    pub root_key: String,
    /// Optional human-readable justification, surfaced in diagnostics
    /// only — not matched against.
    pub reason: Option<String>,
}

impl ContextSuppressRule {
    /// Test whether `file_key` (a forward-slash, normalised path) is
    /// matched by this rule's globs. See [`OverrideBlock::is_match`].
    pub fn is_match(&self, file_key: &str) -> bool {
        match relativize(&self.root_key, file_key) {
            Some(relative) => self.globset.is_match(relative),
            None => false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{OptionDefault, OptionKind, OptionSpec, RawOptionValue, Severity};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    const SCHEMA: &[OptionSpec] = &[OptionSpec {
        name: "limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(80),
        doc: "max line length",
    }];

    #[test]
    fn relativize_does_not_strip_a_root_key_that_is_only_a_string_prefix() {
        // CD-225/CD-226 regression: `/repo-backup/...` must not be
        // mistaken for a path under `/repo` just because "/repo" is a
        // string prefix of "/repo-backup" — the missing separator means
        // it's really a sibling directory, so relativize now returns
        // `None` (not the untouched absolute path) for it.
        assert_eq!(relativize("/repo", "/repo-backup/src/legacy/a.ts"), None);
        // The genuinely-nested case still strips correctly.
        assert_eq!(
            relativize("/repo", "/repo/src/legacy/a.ts"),
            Some("src/legacy/a.ts")
        );
        assert_eq!(relativize("/repo", "/repo"), Some(""));
    }

    #[test]
    fn relativize_treats_a_trailing_slash_root_key_the_same_as_no_trailing_slash() {
        // CD-231: a root_key of "/repo/" must relativize identically to
        // "/repo" — both the exact-root match and a nested file.
        assert_eq!(relativize("/repo/", "/repo"), Some(""));
        assert_eq!(
            relativize("/repo/", "/repo/src/legacy/a.ts"),
            Some("src/legacy/a.ts")
        );
    }

    #[test]
    fn relativize_rejects_a_windows_drive_letter_sibling_directory() {
        // CD-232: `path_key` lowercases and forward-slashes Windows
        // paths, so a sibling directory looks like
        // "c:/repo-backup/..." — a leading-`/`-only out-of-root check
        // let this through as "already relative", reopening the
        // CD-226 leak on Windows. The genuinely-nested case must still
        // strip correctly.
        assert_eq!(
            relativize("c:/repo", "c:/repo-backup/src/legacy/a.ts"),
            None
        );
        assert_eq!(
            relativize("c:/repo", "c:/repo/src/legacy/a.ts"),
            Some("src/legacy/a.ts")
        );
    }

    #[test]
    fn relativize_rejects_a_relative_key_that_escapes_the_root_upward() {
        // CD-232: a `../`-prefixed file_key steps out of root_key
        // entirely and must not be treated as project-relative.
        assert_eq!(relativize("/repo", "../sibling/legacy/a.ts"), None);
        assert_eq!(relativize("/repo", ".."), None);
    }

    fn glob_for(pattern: &str) -> globset::GlobSet {
        let mut builder = globset::GlobSetBuilder::new();
        builder.add(
            globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .expect("valid glob"),
        );
        builder.build().expect("valid globset")
    }

    #[test]
    fn override_block_is_match_does_not_leak_into_a_sibling_directory() {
        // CD-225: same repro at the OverrideBlock::is_match level — a
        // rule scoped to "/repo" must not match a file under the
        // sibling "/repo-backup" directory.
        let block = OverrideBlock {
            paths: vec!["legacy/**".into()],
            globset: glob_for("legacy/**"),
            root_key: "/repo".into(),
            checks: BTreeMap::new(),
        };
        assert!(!block.is_match("/repo-backup/legacy/a.ts"));
        assert!(block.is_match("/repo/legacy/a.ts"));
    }

    #[test]
    fn override_block_is_match_does_not_leak_into_a_sibling_directory_via_leading_glob() {
        // CD-226: a `**/…` pattern must not match straight through the
        // out-of-root absolute-path prefix that CD-225 stopped stripping
        // into a relative path but didn't stop from reaching the
        // globset unmodified.
        let block = OverrideBlock {
            paths: vec!["**/legacy/**".into()],
            globset: glob_for("**/legacy/**"),
            root_key: "/repo".into(),
            checks: BTreeMap::new(),
        };
        assert!(!block.is_match("/repo-backup/legacy/a.ts"));
        assert!(block.is_match("/repo/src/legacy/a.ts"));
    }

    #[test]
    fn context_suppress_rule_is_match_does_not_leak_into_a_sibling_directory() {
        let rule = ContextSuppressRule {
            check_id: "Context.Precedent".into(),
            paths: vec!["legacy/**".into()],
            globset: glob_for("legacy/**"),
            root_key: "/repo".into(),
            reason: None,
        };
        assert!(!rule.is_match("/repo-backup/legacy/a.ts"));
        assert!(rule.is_match("/repo/legacy/a.ts"));
    }

    #[test]
    fn context_suppress_rule_is_match_does_not_leak_into_a_sibling_directory_via_leading_glob() {
        // CD-226: same repro as the OverrideBlock case above.
        let rule = ContextSuppressRule {
            check_id: "Context.Precedent".into(),
            paths: vec!["**/legacy/**".into()],
            globset: glob_for("**/legacy/**"),
            root_key: "/repo".into(),
            reason: None,
        };
        assert!(!rule.is_match("/repo-backup/legacy/a.ts"));
        assert!(rule.is_match("/repo/src/legacy/a.ts"));
    }

    #[test]
    fn parse_minimal_config() {
        let raw = r#"
[checks."Readability.MaxLineLength"]
limit = 120
"#;
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
        let bag = cfg
            .checks
            .get("Readability.MaxLineLength")
            .expect("present");
        assert_eq!(bag.get("limit"), Some(&RawOptionValue::Int(120)));
    }

    #[test]
    fn engine_type_aware_defaults_to_enabled() {
        // No [engine] table → None → enabled.
        let cfg = loader::parse(Path::new("test.toml"), "").expect("parse");
        assert_eq!(cfg.engine_type_aware, None);
        assert!(cfg.type_aware_enabled());
    }

    #[test]
    fn engine_type_aware_false_opts_out() {
        let raw = r#"
[engine]
type_aware = false
"#;
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_type_aware, Some(false));
        assert!(!cfg.type_aware_enabled());
    }

    #[test]
    fn engine_type_aware_true_is_enabled() {
        let raw = r#"
[engine]
type_aware = true
"#;
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_type_aware, Some(true));
        assert!(cfg.type_aware_enabled());
    }

    #[test]
    fn engine_extra_extensions_defaults_to_empty() {
        let cfg = loader::parse(Path::new("test.toml"), "").expect("parse");
        assert!(cfg.engine_extra_extensions.is_empty());
    }

    #[test]
    fn engine_extra_extensions_parses_and_strips_leading_dots() {
        let raw = r#"
[engine]
extra_extensions = ["md", ".mdx", ""]
"#;
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
        assert_eq!(cfg.engine_extra_extensions, vec!["md", "mdx"]);
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
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
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
        let cfg = loader::parse(Path::new("test.toml"), raw).expect("parse");
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
    fn parse_context_suppress_block() {
        let raw = r#"
[[context_suppress]]
check_id = "Context.Precedent"
paths = ["src/legacy/**"]
reason = "known false convention, see CD-999"
"#;
        let cfg = loader::parse(Path::new("cofferdam.toml"), raw).expect("parse");
        assert_eq!(cfg.context_suppress.len(), 1);
        let rule = &cfg.context_suppress[0];
        assert_eq!(rule.check_id, "Context.Precedent");
        assert_eq!(rule.paths, vec!["src/legacy/**"]);
        assert_eq!(
            rule.reason.as_deref(),
            Some("known false convention, see CD-999")
        );

        let root = &rule.root_key;
        assert!(rule.is_match(&format!("{root}/src/legacy/foo.ts")));
        assert!(!rule.is_match(&format!("{root}/src/current/foo.ts")));
    }

    #[test]
    fn no_context_suppress_yields_empty_vec() {
        let cfg = loader::parse(Path::new("cofferdam.toml"), "").expect("parse");
        assert!(cfg.context_suppress.is_empty());
    }

    #[test]
    fn context_suppress_reason_is_optional() {
        let raw = r#"
[[context_suppress]]
check_id = "Context.Knowledge"
paths = ["docs/**"]
"#;
        let cfg = loader::parse(Path::new("cofferdam.toml"), raw).expect("parse");
        assert_eq!(cfg.context_suppress[0].reason, None);
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
        let cfg = loader::parse(Path::new("cofferdam.toml"), raw).expect("parse");
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
        let cfg = loader::parse(Path::new("cofferdam.toml"), raw).expect("parse");
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
        let cfg = loader::parse(Path::new("cofferdam.toml"), raw).expect("parse");
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
        let err = loader::parse(Path::new("cofferdam.toml"), raw).unwrap_err();
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
        let err = loader::parse(Path::new("cofferdam.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedValue { .. }));
    }

    #[test]
    fn no_overrides_yields_empty_vec() {
        let cfg = loader::parse(Path::new("cofferdam.toml"), "").expect("parse");
        assert!(cfg.overrides.is_empty());
    }

    #[test]
    fn unsupported_value_errors() {
        let raw = r#"
[checks."Foo.Bar"]
weird = 1.5
"#;
        let err = loader::parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedValue { .. }));
    }

    #[test]
    fn bad_severity_string_is_rejected() {
        let raw = r#"
[checks."X.Y"]
severity = "extreme"
"#;
        let err = loader::parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::BadSeverity { .. }));
    }

    #[test]
    fn non_string_severity_is_rejected() {
        let raw = r#"
[checks."X.Y"]
severity = 5
"#;
        let err = loader::parse(Path::new("test.toml"), raw).unwrap_err();
        assert!(matches!(err, ConfigError::SeverityNotString { .. }));
    }

    #[test]
    fn missing_top_level_checks_yields_empty_config() {
        let cfg = loader::parse(Path::new("test.toml"), "").expect("parse");
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
            budgets: BTreeMap::new(),
            engine_extra_extensions: Vec::new(),
            context_suppress: Vec::new(),
            unknown_keys: Vec::new(),
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
            budgets: BTreeMap::new(),
            engine_extra_extensions: Vec::new(),
            context_suppress: Vec::new(),
            unknown_keys: Vec::new(),
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
            budgets: BTreeMap::new(),
            engine_extra_extensions: Vec::new(),
            context_suppress: Vec::new(),
            unknown_keys: Vec::new(),
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
            budgets: BTreeMap::new(),
            engine_extra_extensions: Vec::new(),
            context_suppress: Vec::new(),
            unknown_keys: Vec::new(),
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
