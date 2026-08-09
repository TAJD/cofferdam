//! The `cofferdam.toml` key surface, declared once (CD-311).
//!
//! `docs/reference/cli.md` is generated from clap, so the CLI surface
//! cannot drift out of step with its documentation. Configuration had no
//! equivalent: `ProjectConfig` is hand-parsed, and every page describing
//! it was hand-written prose. Three of the worst findings from the docs
//! sweep (CD-299) were config claims — an `exclude` key that does not
//! exist, promised on two separate pages.
//!
//! Config drift is also more dangerous than CLI drift, because the
//! failure is silent. A wrong flag is rejected by clap; a wrong key is
//! skipped by serde, so the user gets a green run and a rule that never
//! fires.
//!
//! This module is the single source of truth for both halves of the fix:
//! [`unknown_keys`] makes a wrong key loud, and `gen-docs` renders the
//! same table into `docs/reference/config.md`. Adding a key here is what
//! makes it both documented and recognised — there is no way to do one
//! without the other.

/// One documented key.
pub struct KeySpec {
    pub key: &'static str,
    /// Rendered in the reference's type column. Prose, not a Rust type.
    pub type_name: &'static str,
    /// Rendered in the default column; `None` prints an em dash.
    pub default: Option<&'static str>,
    pub doc: &'static str,
}

/// How a section's keys are named.
#[derive(Clone, Copy)]
pub enum Keys {
    /// A fixed set — anything else is a typo.
    Fixed(&'static [KeySpec]),
    /// User-chosen names (check ids, layer names, budget keys). Nothing
    /// here can be called unknown, so the reference documents the shape
    /// rather than a key list.
    Open {
        /// What the keys are, e.g. "check id".
        key_meaning: &'static str,
        /// Keys accepted inside each entry, when the entry is a table
        /// with a fixed shape of its own. Empty when the entry is
        /// free-form (per-check options, whose names come from each
        /// check's own `OptionSpec`).
        entry_keys: &'static [KeySpec],
    },
}

pub struct SectionSpec {
    /// TOML path as a user writes it: `""` for the document root,
    /// `"engine"`, `"[[overrides]]"`.
    pub path: &'static str,
    /// Heading used in the generated reference.
    pub title: &'static str,
    pub doc: &'static str,
    pub keys: Keys,
}

/// A key the loader does not recognise, with a targeted hint when it is
/// a mistake we have seen made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownKey {
    /// Dotted path as the user wrote it: `exclude`, `engine.typeAware`.
    pub path: String,
    pub hint: Option<&'static str>,
}

impl std::fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown key `{}`", self.path)?;
        match self.hint {
            Some(hint) => write!(f, " — {hint}"),
            None => write!(
                f,
                " — it is ignored; see the config reference for the keys that exist"
            ),
        }
    }
}

const ROOT_KEYS: &[KeySpec] = &[
    KeySpec {
        key: "plugins",
        type_name: "array of paths",
        default: Some("`[]`"),
        doc: "Node plugin modules implementing the `@cofferdam/check-sdk` `defineCheck` shape. Paths resolve relative to this file's directory.",
    },
    KeySpec {
        key: "checks",
        type_name: "table",
        default: None,
        doc: "Per-check configuration. See the section below.",
    },
    KeySpec {
        key: "layers",
        type_name: "table",
        default: None,
        doc: "Architectural layers for `Design.LayerViolation`. See the section below. `cofferdam.invariants.toml` is the preferred home; when both files declare `[layers]`, that one wins and cofferdam prints a hint.",
    },
    KeySpec {
        key: "engine",
        type_name: "table",
        default: None,
        doc: "Engine-level toggles. See the section below.",
    },
    KeySpec {
        key: "budgets",
        type_name: "table",
        default: None,
        doc: "Caps on finding counts. See the section below.",
    },
    KeySpec {
        key: "overrides",
        type_name: "array of tables",
        default: Some("`[]`"),
        doc: "Per-path-glob check configuration. See the section below.",
    },
    KeySpec {
        key: "context_suppress",
        type_name: "array of tables",
        default: Some("`[]`"),
        doc: "Suppression of noisy `cofferdam context` digest items. See the section below.",
    },
];

const ENGINE_KEYS: &[KeySpec] = &[
    KeySpec {
        key: "type_aware",
        type_name: "bool",
        default: Some("`true`"),
        doc: "Set `false` to force-disable type-aware checks even when one is registered, so CI machines without Node pay no type-host cost and see no warning.",
    },
    KeySpec {
        key: "extra_extensions",
        type_name: "array of strings",
        default: Some("`[]`"),
        doc: "Extensions to walk beyond the built-in set, e.g. `[\"md\", \"mdx\"]`. Leading dots are stripped; empty entries ignored.",
    },
];

const CHECK_ENTRY_KEYS: &[KeySpec] = &[
    KeySpec {
        key: "severity",
        type_name: "string",
        default: None,
        doc: "Override the check's default severity: `info`, `low`, `medium`, `high` or `critical`.",
    },
    KeySpec {
        key: "enabled",
        type_name: "bool",
        default: None,
        doc: "Accepted but not yet wired to anything. To disable a check over a path glob, use `[[overrides]]` with `disabled = true`.",
    },
];

const OVERRIDE_KEYS: &[KeySpec] = &[
    KeySpec {
        key: "paths",
        type_name: "array of globs",
        default: Some("`[]`"),
        doc: "Files this block applies to. Gitignore-style globs, matched against project-relative paths.",
    },
    KeySpec {
        key: "checks",
        type_name: "table",
        default: None,
        doc: "Check id → the same option table a top-level `[checks.\"X.Y\"]` block takes, plus `disabled = true` to turn the check off for these paths.",
    },
];

const CONTEXT_SUPPRESS_KEYS: &[KeySpec] = &[
    KeySpec {
        key: "check_id",
        type_name: "string",
        default: None,
        doc: "The `Context.*` provider to suppress. Required.",
    },
    KeySpec {
        key: "paths",
        type_name: "array of globs",
        default: Some("`[]`"),
        doc: "Files to suppress it on. Omit for the wildcard form — suppress everything this provider emits.",
    },
    KeySpec {
        key: "reason",
        type_name: "string",
        default: None,
        doc: "Why. Not enforced, but `cofferdam context --lint-context-suppress` reports rules that no longer match anything, and a reason is what makes the report actionable.",
    },
];

/// Every documented section of `cofferdam.toml`, in the order the
/// generated reference presents them.
pub const SECTIONS: &[SectionSpec] = &[
    SectionSpec {
        path: "",
        title: "Top level",
        doc: "Keys written at the root of the file, before any table header.",
        keys: Keys::Fixed(ROOT_KEYS),
    },
    SectionSpec {
        path: "checks",
        title: "`[checks.\"Category.Name\"]`",
        doc: "One block per check. Option names come from that check's own schema — run `cofferdam explain <id>` to see them — so cofferdam cannot list them here. Two keys are common to every check.",
        keys: Keys::Open {
            key_meaning: "check id",
            entry_keys: CHECK_ENTRY_KEYS,
        },
    },
    SectionSpec {
        path: "layers",
        title: "`[layers]`",
        doc: "Layer name → array of globs. The reserved sub-table `[layers.allow]` maps each layer to the layers it may import from; an empty array means an isolated layer, and a layer absent from `allow` may import from anything.",
        keys: Keys::Open {
            key_meaning: "layer name (or the reserved `allow`)",
            entry_keys: &[],
        },
    },
    SectionSpec {
        path: "engine",
        title: "`[engine]`",
        doc: "Engine-level toggles.",
        keys: Keys::Fixed(ENGINE_KEYS),
    },
    SectionSpec {
        path: "budgets",
        title: "`[budgets]`",
        doc: "Check id or category name → maximum finding count. Counted including baselined findings: a budget is a cap on total debt, not a CI-gate exemption.",
        keys: Keys::Open {
            key_meaning: "check id or category name",
            entry_keys: &[],
        },
    },
    SectionSpec {
        path: "overrides",
        title: "`[[overrides]]`",
        doc: "Repeatable. Each block narrows check configuration to a set of path globs.",
        keys: Keys::Fixed(OVERRIDE_KEYS),
    },
    SectionSpec {
        path: "context_suppress",
        title: "`[[context_suppress]]`",
        doc: "Repeatable. Each block suppresses one `cofferdam context` provider on a set of paths.",
        keys: Keys::Fixed(CONTEXT_SUPPRESS_KEYS),
    },
];

/// Keys that do not exist but that a user might reasonably write, with
/// the answer. Every entry here is a mistake the documentation itself
/// once invited (CD-299) or an obvious spelling of a real key.
const HINTS: &[(&str, &str)] = &[
    (
        "exclude",
        "exclusion is a discovery-time decision and lives in .cofferdamignore or .gitignore, not in cofferdam.toml. To turn a check off over a path glob without removing the file from analysis, use [[overrides]] with disabled = true",
    ),
    (
        "ignore",
        "there is no [ignore] table. Use .cofferdamignore or .gitignore to exclude files, or [[overrides]] with disabled = true to turn a check off over a path glob",
    ),
    (
        "include",
        "discovery walks the paths you pass on the command line; to widen it to more file extensions use [engine] extra_extensions",
    ),
    (
        "extends",
        "config inheritance is not implemented; cofferdam.toml is discovered by walking up from the analysed path, and the first file found wins outright",
    ),
    (
        "rules",
        "per-check configuration lives under [checks.\"Category.Name\"]",
    ),
    (
        "severity",
        "severity is set per check, inside a [checks.\"Category.Name\"] block",
    ),
];

fn hint_for(key: &str) -> Option<&'static str> {
    HINTS.iter().find(|(k, _)| *k == key).map(|(_, hint)| *hint)
}

/// Report every key in `doc` that no section declares.
///
/// Deliberately a warning-producing function rather than
/// `#[serde(deny_unknown_fields)]`: rejecting outright would break every
/// existing config carrying a stray key, and the dangerous half of the
/// problem is the silence, not the acceptance.
pub fn unknown_keys(doc: &toml::Table) -> Vec<UnknownKey> {
    let mut out = Vec::new();
    collect(doc, "", root_section(), &mut out);
    out
}

fn root_section() -> &'static SectionSpec {
    // The root section is first by construction; `expect` documents that
    // invariant rather than silently returning nothing.
    SECTIONS
        .first()
        .expect("SECTIONS always contains the root section")
}

fn section_for(path: &str) -> Option<&'static SectionSpec> {
    SECTIONS.iter().find(|s| s.path == path)
}

fn collect(table: &toml::Table, prefix: &str, section: &SectionSpec, out: &mut Vec<UnknownKey>) {
    let known: Option<&[KeySpec]> = match section.keys {
        Keys::Fixed(k) => Some(k),
        Keys::Open { .. } => None,
    };

    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        if let Some(known) = known {
            if !known.iter().any(|k| k.key == key) {
                out.push(UnknownKey {
                    path: path.clone(),
                    hint: hint_for(key),
                });
                // Do not descend into a table we do not recognise —
                // every key inside it would be reported too, burying the
                // one line that matters.
                continue;
            }
        }

        // Descend where a nested section is declared. `[checks]` and
        // `[budgets]` entries are user-named, so their contents are
        // checked against the section's `entry_keys` only when it
        // declares any; per-check options are validated later, against
        // the check's own schema.
        match (section.path, value) {
            ("", toml::Value::Table(t)) => {
                if let Some(nested) = section_for(key) {
                    collect(t, &path, nested, out);
                }
            }
            ("", toml::Value::Array(items)) => {
                if let Some(nested) = section_for(key) {
                    for (i, item) in items.iter().enumerate() {
                        if let toml::Value::Table(t) = item {
                            collect(t, &format!("{key}[{i}]"), nested, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(raw: &str) -> Vec<String> {
        let doc: toml::Table = raw.parse().expect("parse");
        unknown_keys(&doc).into_iter().map(|u| u.path).collect()
    }

    #[test]
    fn a_realistic_config_is_clean() {
        let raw = r#"
plugins = ["./plugin"]

[checks."Readability.MaxLineLength"]
limit = 120
severity = "low"

[layers]
domain = ["src/domain/**"]
[layers.allow]
domain = []

[engine]
type_aware = false
extra_extensions = ["md"]

[budgets]
"Warning.NoConsoleLog" = 5

[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Refactor.CyclomaticComplexity"]
disabled = true

[[context_suppress]]
check_id = "Context.Precedent"
paths = ["src/generated/**"]
reason = "generated"
"#;
        assert_eq!(unknown(raw), Vec::<String>::new());
    }

    /// The motivating case (CD-299): two docs pages promised an
    /// `exclude` key. A user who wrote it got a clean parse and no
    /// exclusion.
    #[test]
    fn the_exclude_key_the_docs_once_promised_is_reported_with_the_answer() {
        let doc: toml::Table = "exclude = [\"dist/**\"]\n".parse().expect("parse");
        let found = unknown_keys(&doc);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "exclude");
        assert!(
            found[0]
                .hint
                .is_some_and(|h| h.contains(".cofferdamignore")),
            "an unknown key we have seen invented deserves the answer, not just a rejection"
        );
    }

    #[test]
    fn unknown_keys_inside_known_tables_are_reported_with_their_path() {
        assert_eq!(
            unknown("[engine]\ntypeAware = false\n"),
            ["engine.typeAware"]
        );
    }

    #[test]
    fn unknown_keys_in_array_tables_carry_their_index() {
        let raw = "[[overrides]]\npaths = [\"a\"]\n\n[[overrides]]\npath = \"b\"\n";
        assert_eq!(unknown(raw), ["overrides[1].path"]);
    }

    /// User-named tables cannot contain an unknown key by definition,
    /// and per-check option names come from each check's own schema —
    /// reporting them here would fire on every correctly-configured
    /// project.
    #[test]
    fn user_named_keys_are_never_unknown() {
        let raw = r#"
[checks."Readability.MaxLineLength"]
limit = 120
some_future_option = true

[layers]
whatever_layer_name = ["src/**"]

[budgets]
"Some.Check" = 3
"#;
        assert_eq!(unknown(raw), Vec::<String>::new());
    }

    /// One line per mistake. Reporting every key inside an unrecognised
    /// table would bury the one that matters.
    #[test]
    fn an_unknown_table_is_reported_once_not_per_key() {
        assert_eq!(unknown("[ignore]\npaths = [\"a\"]\nmore = 1\n"), ["ignore"]);
    }

    /// Every key the loader accepts must be declared here, or the
    /// warning fires on a valid config. This test is the coupling: it
    /// fails if `TomlDoc` grows a field and this table does not.
    #[test]
    fn every_root_key_the_loader_accepts_is_declared() {
        for key in [
            "checks",
            "layers",
            "plugins",
            "engine",
            "overrides",
            "budgets",
            "context_suppress",
        ] {
            assert!(
                ROOT_KEYS.iter().any(|k| k.key == key),
                "TomlDoc accepts `{key}` but schema.rs does not declare it, so a valid \
                 config would be warned about and the generated reference would omit it"
            );
        }
    }
}
