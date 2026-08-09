//! `cofferdam invariants` — show, validate and normalise the resolved
//! architectural spec (CD-308).
//!
//! The spec is the load-bearing config surface: it merges `cofferdam.toml`
//! `[layers]` with `cofferdam.invariants.toml`, resolves globs, and
//! validates a predicate DSL whose failure aborts the whole run. Until
//! this subcommand existed, a user whose rule was not firing had no way to
//! ask the CLI what it had actually loaded — only the MCP server could
//! answer, which inverted the usual "MCP wraps the CLI" contract.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cofferdam_core::invariants::{InvariantsSpec, FILE_NAME};
use cofferdam_engine::config::{self as cfg, ProjectConfig};
use serde::Serialize;

/// Where the layers actually in force came from. Worth reporting on its
/// own: `cofferdam.invariants.toml` replaces `cofferdam.toml`'s `[layers]`
/// wholesale, and a user staring at a rule that will not fire is often
/// looking at the file that lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayersSource {
    Invariants,
    CofferdamToml,
    None,
}

impl LayersSource {
    fn label(self) -> &'static str {
        match self {
            Self::Invariants => FILE_NAME,
            Self::CofferdamToml => "cofferdam.toml",
            Self::None => "(none declared)",
        }
    }
}

/// The resolved spec as reported to the user. Deliberately not
/// `InvariantsSpec` itself: the answer to "what is in force?" includes
/// which files were read, where the layers came from, and the load
/// warnings — none of which live on the spec.
#[derive(Debug, Serialize)]
pub struct ResolvedSpec {
    pub cofferdam_toml: Option<PathBuf>,
    pub invariants_toml: Option<PathBuf>,
    pub schema_version: Option<String>,
    pub schema_version_explicit: bool,
    pub schema_version_deprecated: bool,
    pub layers_source: LayersSource,
    pub layers: BTreeMap<String, Vec<String>>,
    pub layers_allow: BTreeMap<String, Vec<String>>,
    pub public_api_exports: Vec<String>,
    pub boundaries: BTreeMap<String, BoundaryOut>,
    pub invariants: BTreeMap<String, InvariantOut>,
    pub scripted: BTreeMap<String, ScriptedOut>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BoundaryOut {
    pub frozen: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InvariantOut {
    pub forbid_imports: Vec<String>,
    pub require_imports: Vec<String>,
    pub from_layers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScriptedOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forbid: Option<String>,
    pub message: String,
}

impl ResolvedSpec {
    /// True when nothing architectural is declared anywhere. `show`
    /// reports this rather than printing an empty skeleton, because
    /// "you have no spec" and "your spec is empty" send the same user to
    /// different fixes.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
            && self.public_api_exports.is_empty()
            && self.boundaries.is_empty()
            && self.invariants.is_empty()
            && self.scripted.is_empty()
    }
}

/// Resolve the spec exactly the way `cofferdam check` does, so `show`
/// cannot report something the run would not use.
pub fn resolve(
    explicit_config: Option<&Path>,
    start: &Path,
    no_config: bool,
) -> Result<ResolvedSpec, String> {
    let (config, toml_path, diags) =
        cfg::resolve_with_invariants(explicit_config, start, no_config)
            .map_err(|e| e.to_string())?;

    let config = config.unwrap_or_default();
    Ok(build(&config, toml_path, diags.warnings))
}

fn build(
    config: &ProjectConfig,
    toml_path: Option<PathBuf>,
    warnings: Vec<String>,
) -> ResolvedSpec {
    let spec = config.invariants.as_ref();
    let spec_has_layers = spec.is_some_and(|s| !s.layers.is_empty());
    let layers_source = if spec_has_layers {
        LayersSource::Invariants
    } else if config.layers.is_some() {
        LayersSource::CofferdamToml
    } else {
        LayersSource::None
    };

    // Report the layers actually in force — `config.layers` after the
    // merge — not the invariants spec's own copy, which is empty in the
    // cofferdam.toml-only case.
    let (layers, layers_allow) = match config.layers.as_ref() {
        Some(l) => (l.layers.clone(), l.allow.clone()),
        None => (BTreeMap::new(), BTreeMap::new()),
    };

    ResolvedSpec {
        cofferdam_toml: toml_path,
        invariants_toml: spec.map(|s| s.project_root.join(FILE_NAME)),
        schema_version: spec.map(|s| s.schema_version.to_string()),
        schema_version_explicit: spec.is_some_and(|s| s.schema_version_explicit),
        schema_version_deprecated: spec.is_some_and(|s| s.schema_version_deprecated),
        layers_source,
        layers,
        layers_allow,
        public_api_exports: spec
            .map(|s| s.public_api.exports.clone())
            .unwrap_or_default(),
        boundaries: spec
            .map(|s| {
                s.boundaries
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            BoundaryOut {
                                frozen: v.frozen,
                                reason: v.reason.clone(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        invariants: spec
            .map(|s| {
                s.invariants
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            InvariantOut {
                                forbid_imports: v.forbid_imports.clone(),
                                require_imports: v.require_imports.clone(),
                                from_layers: v.from_layers.clone(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        scripted: spec
            .map(|s| {
                s.scripted
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            ScriptedOut {
                                when: v.when.clone(),
                                require: v.require.clone(),
                                forbid: v.forbid.clone(),
                                message: v.message.clone(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        warnings,
    }
}

/// Markdown-ish text rendering, matching the register `cofferdam context`
/// uses for its digest.
pub fn render_text(r: &ResolvedSpec) -> String {
    let mut out = String::new();
    out.push_str("# Resolved invariants spec\n\n");

    match (&r.cofferdam_toml, &r.invariants_toml) {
        (None, None) => out.push_str("No config file found.\n"),
        _ => {
            if let Some(p) = &r.cofferdam_toml {
                out.push_str(&format!("cofferdam.toml:      {}\n", p.display()));
            }
            if let Some(p) = &r.invariants_toml {
                out.push_str(&format!("invariants:          {}\n", p.display()));
            }
        }
    }
    if let Some(v) = &r.schema_version {
        let how = if r.schema_version_explicit {
            "declared"
        } else {
            "assumed — the file does not declare one"
        };
        out.push_str(&format!("schema_version:      {v} ({how})\n"));
        if r.schema_version_deprecated {
            out.push_str("                     deprecated — older than the current version\n");
        }
    }
    out.push('\n');

    if r.is_empty() {
        out.push_str("Nothing declared: no layers, public API, boundaries or invariants.\n");
    }

    out.push_str(&format!("## Layers — from {}\n\n", r.layers_source.label()));
    if r.layers.is_empty() {
        out.push_str("(none)\n");
    } else {
        for (name, globs) in &r.layers {
            out.push_str(&format!("- {name}: {}\n", globs.join(", ")));
            if let Some(allow) = r.layers_allow.get(name) {
                let rendered = if allow.is_empty() {
                    "(isolated — may import from no layer)".to_string()
                } else {
                    allow.join(", ")
                };
                out.push_str(&format!("    may import: {rendered}\n"));
            }
        }
    }
    out.push('\n');

    out.push_str("## Public API\n\n");
    if r.public_api_exports.is_empty() {
        out.push_str("(none)\n");
    } else {
        for e in &r.public_api_exports {
            out.push_str(&format!("- {e}\n"));
        }
    }
    out.push('\n');

    out.push_str("## Boundaries\n\n");
    if r.boundaries.is_empty() {
        out.push_str("(none)\n");
    } else {
        for (glob, b) in &r.boundaries {
            let frozen = if b.frozen { "frozen" } else { "not frozen" };
            out.push_str(&format!("- {glob}: {frozen}\n"));
            if let Some(reason) = &b.reason {
                out.push_str(&format!("    reason: {reason}\n"));
            }
        }
    }
    out.push('\n');

    out.push_str("## Invariants\n\n");
    if r.invariants.is_empty() && r.scripted.is_empty() {
        out.push_str("(none)\n");
    }
    for (name, inv) in &r.invariants {
        out.push_str(&format!("- {name}\n"));
        if !inv.from_layers.is_empty() {
            out.push_str(&format!(
                "    from layers: {}\n",
                inv.from_layers.join(", ")
            ));
        }
        if !inv.forbid_imports.is_empty() {
            out.push_str(&format!("    forbid: {}\n", inv.forbid_imports.join(", ")));
        }
        if !inv.require_imports.is_empty() {
            out.push_str(&format!(
                "    require: {}\n",
                inv.require_imports.join(", ")
            ));
        }
    }
    for (name, s) in &r.scripted {
        out.push_str(&format!("- {name} (scripted)\n"));
        if let Some(w) = &s.when {
            out.push_str(&format!("    when: {w}\n"));
        }
        if let Some(p) = &s.require {
            out.push_str(&format!("    require: {p}\n"));
        }
        if let Some(p) = &s.forbid {
            out.push_str(&format!("    forbid: {p}\n"));
        }
        out.push_str(&format!("    message: {}\n", s.message));
    }

    if !r.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for w in &r.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }
    out
}

/// Canonical TOML serialisation of the spec, per `docs/schema-versioning.md`.
/// Only the `cofferdam.invariants.toml` half round-trips: `cofferdam.toml`
/// layers are a different file with a different schema, and silently
/// promoting them here would emit a file the user did not write.
pub fn normalize(spec: &InvariantsSpec) -> Result<String, String> {
    cofferdam_core::invariants::to_toml_string(spec).map_err(|e| e.to_string())
}
