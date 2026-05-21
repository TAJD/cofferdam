//! The canonical graph's closed-core taxonomy.
//!
//! Every node and edge in the project graph is one of these enum
//! variants. Built-ins compile against the closed set; adapters use
//! the `Extension { ns, kind, attrs }` variant to add domain-specific
//! types under their namespace.
//!
//! The taxonomy is fixed by [ADR 0001](../../../docs/decisions/0001-canonical-graph-schema.md).
//! Changes here are schema bumps under the MAJOR.MINOR policy in
//! [docs/schema-versioning.md](../../../docs/schema-versioning.md):
//!
//! - **MINOR** — new closed-core variants (e.g. additional
//!   `SymbolKind` shapes, additional `EdgeKind` relations). Reading
//!   code MUST tolerate unknown variants when decoding from a
//!   newer-build snapshot; until cp5 wires schema versioning, the
//!   matches stay exhaustive.
//! - **MAJOR** — semantic shifts in existing variants, removal of
//!   variants, renames. Reserved for cd-9hp.9 + 1 if it ever lands.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::value::AttrMap;

// ---------------------------------------------------------------------------
// Node taxonomy
// ---------------------------------------------------------------------------

/// One node in the canonical graph. The closed core covers the
/// import / export / layer surface that the existing built-ins need;
/// adapter-defined kinds ride on [`NodeKind::Extension`].
///
/// Variants carry their identifying payload directly so identity
/// computation in [`crate::id::compute_node_id`] doesn't need to
/// reach back into a separate attribute store for the common case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
// JSON wire shape: `{"type": "<variant>", ...payload}`. We use `type`
// rather than `kind` for the tag because several variants (`Symbol`,
// `ExportsAs`) carry a `kind` field of their own, and serde forbids
// the internal-tag name from colliding with any variant field.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// A source file under analysis. Identified by absolute path.
    /// `lang` is the language tag the engine assigned (e.g.
    /// `"typescript"`, `"rust"`); same value as
    /// [`cofferdam_core::Language::id_str`] would emit.
    File { path: PathBuf, lang: SmolStr },
    /// A declared symbol — function, class, interface, type, variable,
    /// namespace. `name` is the source-level identifier; `kind` picks
    /// the symbol-flavour bucket. Symbols are anchored to the file
    /// that declares them via the [`EdgeKind::DeclaredIn`] edge.
    Symbol { name: SmolStr, kind: SymbolKind },
    /// An import specifier (`'react'`, `'./foo'`, `'@/utils'`). One
    /// shared `Import` node per distinct specifier across the project
    /// — multiple files importing the same module share the same
    /// `Import` node, the per-file edge carries the importing-file
    /// pointer.
    Import { specifier: SmolStr },
    /// A named export. Anchored to the exporting file via
    /// [`EdgeKind::ExportsAs`]. Default exports use the literal name
    /// `"default"`; namespace re-exports use `"*"`.
    Export { name: SmolStr },
    /// A declared architectural layer from `cofferdam.invariants.toml`.
    /// Files belong to at most one layer via
    /// [`EdgeKind::BelongsToLayer`].
    Layer { name: SmolStr },
    /// Adapter-defined node under a namespace (`sql.*`, `iac.*`,
    /// `gql.*`, ...). Built-in code MUST treat the `ns` + `kind` pair
    /// opaquely; the predicate DSL looks them up by string match.
    /// `attrs` carries the adapter's identifying payload.
    Extension {
        ns: SmolStr,
        kind: SmolStr,
        attrs: AttrMap,
    },
}

/// Flavour of a declared [`NodeKind::Symbol`].
///
/// Closed bucket; new TS-side shapes (e.g. `Enum`, `Decorator`) come
/// through MINOR schema bumps. Adapter-defined symbol-like nodes use
/// [`NodeKind::Extension`] instead — `SymbolKind` is reserved for
/// concepts the built-in checks ALREADY reason about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    Type,
    Variable,
    Namespace,
    /// Escape hatch for symbol shapes that don't fit the named
    /// buckets. Prefer adding a new variant under a MINOR bump rather
    /// than reaching for `Other`.
    Other,
}

// ---------------------------------------------------------------------------
// Edge taxonomy
// ---------------------------------------------------------------------------

/// One edge in the canonical graph. Multiple edges between the same
/// two nodes are allowed when their kind differs.
///
/// The closed core matches the verbs in [`cofferdam_core::graph`]
/// today: a file declares symbols, imports modules (as type or as
/// value), exports symbols, and belongs to a layer. Adapter-defined
/// relations ride on [`EdgeKind::Extension`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
// Same `tag = "type"` rationale as [`NodeKind`] — see the note above.
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EdgeKind {
    /// Symbol → File. Anchors a declared symbol to the file that
    /// introduces it.
    DeclaredIn,
    /// File → Import (or File → File when the resolver succeeded).
    /// Value-mode import — the binding is used at runtime, not only
    /// at type position.
    ImportsAsValue,
    /// File → Import (or File → File). Type-only import — TS
    /// `import type` at the statement level, or per-name
    /// `import { type X }`.
    ImportsAsType,
    /// File → Symbol (named/default) or File → File (re-export).
    /// `kind` distinguishes named vs default vs re-export shapes;
    /// see [`ExportKind`].
    ExportsAs { kind: ExportKind },
    /// File → Layer. Set when the file's path matches one of the
    /// declared layer globs in `cofferdam.invariants.toml`. At most
    /// one such edge per file.
    BelongsToLayer,
    /// Adapter-defined edge. See [`NodeKind::Extension`] for the
    /// same convention; built-in code MUST NOT pattern-match on the
    /// `ns` + `kind` pair.
    Extension {
        ns: SmolStr,
        kind: SmolStr,
        attrs: AttrMap,
    },
}

/// Shape of an [`EdgeKind::ExportsAs`] edge.
///
/// Mirrors [`cofferdam_core::graph::ExportKind`]; the duplicate exists
/// because the canonical-graph schema is a closed boundary that
/// shouldn't pull cofferdam-core's wire types into the graph's own
/// public surface. cp3 wires a `From<cofferdam_core::graph::ExportKind>`
/// when `Design.OrphanExport` migrates onto the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    Named,
    Default,
    ReExport,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_attrs() -> AttrMap {
        let mut m = AttrMap::new();
        m.insert(SmolStr::new("k"), crate::Value::from("v"));
        m
    }

    // -----------------------------------------------------------------------
    // Serde round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn node_kind_round_trips_through_json() {
        let cases: Vec<NodeKind> = vec![
            NodeKind::File {
                path: PathBuf::from("/proj/src/foo.ts"),
                lang: SmolStr::new("typescript"),
            },
            NodeKind::Symbol {
                name: SmolStr::new("Widget"),
                kind: SymbolKind::Class,
            },
            NodeKind::Import {
                specifier: SmolStr::new("react"),
            },
            NodeKind::Export {
                name: SmolStr::new("default"),
            },
            NodeKind::Layer {
                name: SmolStr::new("app"),
            },
            NodeKind::Extension {
                ns: SmolStr::new("sql"),
                kind: SmolStr::new("column"),
                attrs: small_attrs(),
            },
        ];
        for case in cases {
            let s = serde_json::to_string(&case).expect("serialize");
            let r: NodeKind = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(case, r, "node round-trip failed (json: {s})");
        }
    }

    #[test]
    fn edge_kind_round_trips_through_json() {
        let cases: Vec<EdgeKind> = vec![
            EdgeKind::DeclaredIn,
            EdgeKind::ImportsAsValue,
            EdgeKind::ImportsAsType,
            EdgeKind::ExportsAs {
                kind: ExportKind::Named,
            },
            EdgeKind::ExportsAs {
                kind: ExportKind::Default,
            },
            EdgeKind::ExportsAs {
                kind: ExportKind::ReExport,
            },
            EdgeKind::BelongsToLayer,
            EdgeKind::Extension {
                ns: SmolStr::new("iac"),
                kind: SmolStr::new("references"),
                attrs: small_attrs(),
            },
        ];
        for case in cases {
            let s = serde_json::to_string(&case).expect("serialize");
            let r: EdgeKind = serde_json::from_str(&s).expect("deserialize");
            assert_eq!(case, r, "edge round-trip failed (json: {s})");
        }
    }

    // -----------------------------------------------------------------------
    // Exhaustiveness — match on every variant compiles. If a future
    // MINOR adds a variant, these tests force the new variant to be
    // considered here so we don't ship a partial match by accident.
    // -----------------------------------------------------------------------

    #[test]
    fn node_kind_match_is_exhaustive() {
        // Constructed to exercise every variant by reaching into a
        // closed match; the test fails to compile if a variant is
        // added without updating this arm.
        fn touch(n: &NodeKind) -> &'static str {
            match n {
                NodeKind::File { .. } => "file",
                NodeKind::Symbol { .. } => "symbol",
                NodeKind::Import { .. } => "import",
                NodeKind::Export { .. } => "export",
                NodeKind::Layer { .. } => "layer",
                NodeKind::Extension { .. } => "extension",
            }
        }
        let _ = touch(&NodeKind::Layer {
            name: SmolStr::new("x"),
        });
    }

    #[test]
    fn edge_kind_match_is_exhaustive() {
        fn touch(e: &EdgeKind) -> &'static str {
            match e {
                EdgeKind::DeclaredIn => "declared_in",
                EdgeKind::ImportsAsValue => "imports_as_value",
                EdgeKind::ImportsAsType => "imports_as_type",
                EdgeKind::ExportsAs { .. } => "exports_as",
                EdgeKind::BelongsToLayer => "belongs_to_layer",
                EdgeKind::Extension { .. } => "extension",
            }
        }
        let _ = touch(&EdgeKind::DeclaredIn);
    }

    #[test]
    fn symbol_kind_match_is_exhaustive() {
        fn touch(s: SymbolKind) -> &'static str {
            match s {
                SymbolKind::Function => "function",
                SymbolKind::Class => "class",
                SymbolKind::Interface => "interface",
                SymbolKind::Type => "type",
                SymbolKind::Variable => "variable",
                SymbolKind::Namespace => "namespace",
                SymbolKind::Other => "other",
            }
        }
        let _ = touch(SymbolKind::Function);
    }

    #[test]
    fn export_kind_match_is_exhaustive() {
        fn touch(e: ExportKind) -> &'static str {
            match e {
                ExportKind::Named => "named",
                ExportKind::Default => "default",
                ExportKind::ReExport => "re_export",
            }
        }
        let _ = touch(ExportKind::Named);
    }
}
