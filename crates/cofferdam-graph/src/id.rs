//! Stable identity for canonical-graph nodes.
//!
//! `NodeId(u64)` is a content-addressed handle derived from
//! `(variant discriminant, identifying attrs)` via sha2 truncated to
//! 64 bits. Two nodes with the same identifying payload hash to the
//! same id; the engine deduplicates on this id when materialising the
//! graph from per-file extraction passes.
//!
//! # Why sha2, not `DefaultHasher`
//!
//! `std::collections::hash_map::DefaultHasher` is not guaranteed
//! stable across Rust versions, and even within a version its output
//! changes if the standard library tweaks the seed strategy. The
//! canonical graph wants `(payload) → id` to be deterministic both
//! within a run (for dedupe) and across builds (for incremental
//! analysis caches and snapshot tests). sha2 is already a workspace
//! dependency, and the per-node hash cost — a few hundred nanoseconds
//! — is dwarfed by the parse + analyze work that produces the
//! payload.
//!
//! # Why truncate to u64
//!
//! u64 collisions on a project graph with <1M nodes have probability
//! ~10^-7 — well below the rate at which we'd care. If a future
//! workload pushes that, we widen `NodeId` under a MAJOR schema bump.

use std::path::Path;

use sha2::{Digest, Sha256};
use smol_str::SmolStr;

use crate::schema::{EdgeKind, ExportKind, NodeKind, SymbolKind};
use crate::value::{AttrMap, Value};

/// Content-addressed handle for one canonical-graph node.
///
/// Stable within and across cofferdam builds: same payload → same id.
/// Use [`compute_node_id`] to compute one from a [`NodeKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Raw `u64` view. Equivalent to `node_id.0`; provided for
    /// readability in benchmarks and JSON output.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node-{:016x}", self.0)
    }
}

/// Compute the stable [`NodeId`] for a node. Two equivalent
/// [`NodeKind`] payloads produce the same id; ordering of nested
/// `AttrMap` entries is irrelevant because [`AttrMap`] is a
/// `BTreeMap` (deterministic iteration order).
pub fn compute_node_id(node: &NodeKind) -> NodeId {
    let mut h = Sha256::new();
    // Discriminant tag — keeps two payloads that happen to share the
    // same identifying bytes (e.g. a `Layer { name: "app" }` and a
    // `Symbol { name: "app", kind: Function }`) in distinct id spaces.
    h.update(node_discriminant(node).to_le_bytes());
    match node {
        NodeKind::File { path, lang } => {
            hash_path(&mut h, path);
            hash_str(&mut h, lang);
        }
        NodeKind::Symbol { name, kind } => {
            hash_str(&mut h, name);
            h.update([symbol_discriminant(*kind)]);
        }
        NodeKind::Import { specifier } => {
            hash_str(&mut h, specifier);
        }
        NodeKind::Export { name } => {
            hash_str(&mut h, name);
        }
        NodeKind::Layer { name } => {
            hash_str(&mut h, name);
        }
        NodeKind::Extension { ns, kind, attrs } => {
            hash_str(&mut h, ns);
            hash_str(&mut h, kind);
            hash_attrs(&mut h, attrs);
        }
    }
    let digest = h.finalize();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    NodeId(u64::from_le_bytes(buf))
}

// ---------------------------------------------------------------------------
// Discriminant tables — explicit so a future MINOR reorder of an enum
// can't silently re-shuffle ids. Adding a variant gets a brand-new
// tag; existing tags MUST NOT be renumbered without a MAJOR bump.
// ---------------------------------------------------------------------------

fn node_discriminant(n: &NodeKind) -> u32 {
    match n {
        NodeKind::File { .. } => 1,
        NodeKind::Symbol { .. } => 2,
        NodeKind::Import { .. } => 3,
        NodeKind::Export { .. } => 4,
        NodeKind::Layer { .. } => 5,
        NodeKind::Extension { .. } => 6,
    }
}

fn symbol_discriminant(s: SymbolKind) -> u8 {
    match s {
        SymbolKind::Function => 1,
        SymbolKind::Class => 2,
        SymbolKind::Interface => 3,
        SymbolKind::Type => 4,
        SymbolKind::Variable => 5,
        SymbolKind::Namespace => 6,
        SymbolKind::Other => 7,
    }
}

/// Edge discriminant table. Exposed `pub` because cp2 wires it into
/// the graph backend's edge identity (an edge is `(src_id, dst_id,
/// edge_discriminant, attrs)`). Same MAJOR-bump contract as the node
/// table above.
pub fn edge_discriminant(e: &EdgeKind) -> u32 {
    match e {
        EdgeKind::DeclaredIn => 1,
        EdgeKind::ImportsAsValue => 2,
        EdgeKind::ImportsAsType => 3,
        EdgeKind::ExportsAs { .. } => 4,
        EdgeKind::BelongsToLayer => 5,
        EdgeKind::Extension { .. } => 6,
    }
}

/// Discriminant for [`ExportKind`]. Same MAJOR-bump contract as the
/// node and edge tables.
pub fn export_discriminant(e: ExportKind) -> u8 {
    match e {
        ExportKind::Named => 1,
        ExportKind::Default => 2,
        ExportKind::ReExport => 3,
    }
}

// ---------------------------------------------------------------------------
// Hash primitives
// ---------------------------------------------------------------------------

fn hash_str(h: &mut Sha256, s: &SmolStr) {
    h.update((s.len() as u64).to_le_bytes());
    h.update(s.as_bytes());
}

fn hash_path(h: &mut Sha256, p: &Path) {
    let s = p.to_string_lossy();
    // Path separators differ on Windows vs Unix; normalise to forward
    // slashes so the same logical path produces the same id on either
    // host. Matches the normalisation cofferdam-core uses for
    // public_api glob matching.
    let normalised: String = s.replace('\\', "/");
    h.update((normalised.len() as u64).to_le_bytes());
    h.update(normalised.as_bytes());
}

fn hash_attrs(h: &mut Sha256, attrs: &AttrMap) {
    // BTreeMap iteration is deterministic; we still write the entry
    // count so two attribute maps with the same prefix-bytes don't
    // collide (defensive).
    h.update((attrs.len() as u64).to_le_bytes());
    for (k, v) in attrs {
        hash_str(h, k);
        hash_value(h, v);
    }
}

fn hash_value(h: &mut Sha256, v: &Value) {
    match v {
        Value::String(s) => {
            h.update([1u8]);
            hash_str(h, s);
        }
        Value::Int(i) => {
            h.update([2u8]);
            h.update(i.to_le_bytes());
        }
        Value::Bool(b) => {
            h.update([3u8]);
            h.update([*b as u8]);
        }
        Value::List(xs) => {
            h.update([4u8]);
            h.update((xs.len() as u64).to_le_bytes());
            for x in xs {
                hash_value(h, x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn file(p: &str) -> NodeKind {
        NodeKind::File {
            path: PathBuf::from(p),
            lang: SmolStr::new("typescript"),
        }
    }

    // -----------------------------------------------------------------------
    // Stability — same input, same id.
    // -----------------------------------------------------------------------

    #[test]
    fn id_is_stable_for_same_payload() {
        let a = compute_node_id(&file("/proj/src/foo.ts"));
        let b = compute_node_id(&file("/proj/src/foo.ts"));
        assert_eq!(a, b, "same payload must produce same id");
    }

    /// Snapshot-style pin: hard-codes one expected id so a future
    /// refactor of the hash primitives surfaces as a deliberate change
    /// rather than a silent drift. If this fails, audit the change
    /// and bump the schema MAJOR; otherwise update the constant.
    #[test]
    fn id_is_stable_across_builds() {
        let id = compute_node_id(&file("/proj/src/foo.ts"));
        // Snapshot from the initial sha2-truncate impl. If this number
        // changes, audit the diff — accidental drift is a bug;
        // deliberate change is a schema bump. Either way, update the
        // constant after the audit.
        assert_eq!(
            id.as_u64(),
            0x7893_1f33_a4ad_f7d4,
            "node identity drifted — schema bump required"
        );
    }

    // -----------------------------------------------------------------------
    // Distinctness — different payloads, different ids (modulo
    // 1-in-2^64 collision).
    // -----------------------------------------------------------------------

    #[test]
    fn different_paths_yield_different_ids() {
        let a = compute_node_id(&file("/proj/src/foo.ts"));
        let b = compute_node_id(&file("/proj/src/bar.ts"));
        assert_ne!(a, b);
    }

    #[test]
    fn different_variants_with_same_inner_string_are_distinct() {
        // The discriminant tag prevents collision when two variants
        // happen to carry the same identifying bytes.
        let a = compute_node_id(&NodeKind::Layer {
            name: SmolStr::new("app"),
        });
        let b = compute_node_id(&NodeKind::Export {
            name: SmolStr::new("app"),
        });
        assert_ne!(
            a, b,
            "discriminant must split Layer('app') from Export('app')"
        );
    }

    #[test]
    fn windows_path_separators_dont_change_id() {
        // hash_path normalises backslashes to forward slashes so a TS
        // path produced on Windows hashes to the same id as the same
        // logical path on Linux.
        let a = compute_node_id(&NodeKind::File {
            path: PathBuf::from("/proj/src/foo.ts"),
            lang: SmolStr::new("typescript"),
        });
        let b = compute_node_id(&NodeKind::File {
            path: PathBuf::from(r"\proj\src\foo.ts"),
            lang: SmolStr::new("typescript"),
        });
        assert_eq!(a, b, "path separator normalisation must produce equal ids");
    }

    #[test]
    fn symbol_kind_affects_id() {
        let f = NodeKind::Symbol {
            name: SmolStr::new("Widget"),
            kind: SymbolKind::Function,
        };
        let c = NodeKind::Symbol {
            name: SmolStr::new("Widget"),
            kind: SymbolKind::Class,
        };
        assert_ne!(compute_node_id(&f), compute_node_id(&c));
    }

    #[test]
    fn extension_attrs_are_order_independent() {
        // AttrMap is a BTreeMap so iteration order is deterministic;
        // this asserts the determinism translates to a stable id when
        // attrs are inserted in different orders.
        let mut a = AttrMap::new();
        a.insert(SmolStr::new("k1"), Value::from("v1"));
        a.insert(SmolStr::new("k2"), Value::from("v2"));
        let mut b = AttrMap::new();
        b.insert(SmolStr::new("k2"), Value::from("v2"));
        b.insert(SmolStr::new("k1"), Value::from("v1"));

        let na = NodeKind::Extension {
            ns: SmolStr::new("sql"),
            kind: SmolStr::new("column"),
            attrs: a,
        };
        let nb = NodeKind::Extension {
            ns: SmolStr::new("sql"),
            kind: SmolStr::new("column"),
            attrs: b,
        };
        assert_eq!(compute_node_id(&na), compute_node_id(&nb));
    }

    #[test]
    fn extension_different_attrs_are_distinct() {
        let mut a = AttrMap::new();
        a.insert(SmolStr::new("k"), Value::from("a"));
        let mut b = AttrMap::new();
        b.insert(SmolStr::new("k"), Value::from("b"));

        let na = NodeKind::Extension {
            ns: SmolStr::new("sql"),
            kind: SmolStr::new("column"),
            attrs: a,
        };
        let nb = NodeKind::Extension {
            ns: SmolStr::new("sql"),
            kind: SmolStr::new("column"),
            attrs: b,
        };
        assert_ne!(compute_node_id(&na), compute_node_id(&nb));
    }

    // -----------------------------------------------------------------------
    // Discriminant helpers
    // -----------------------------------------------------------------------

    #[test]
    fn edge_discriminants_are_distinct() {
        use std::collections::HashSet;
        let edges = [
            EdgeKind::DeclaredIn,
            EdgeKind::ImportsAsValue,
            EdgeKind::ImportsAsType,
            EdgeKind::ExportsAs {
                kind: ExportKind::Named,
            },
            EdgeKind::BelongsToLayer,
            EdgeKind::Extension {
                ns: SmolStr::new("x"),
                kind: SmolStr::new("y"),
                attrs: AttrMap::new(),
            },
        ];
        let set: HashSet<u32> = edges.iter().map(edge_discriminant).collect();
        assert_eq!(set.len(), edges.len(), "edge discriminants must be unique");
    }

    #[test]
    fn export_discriminants_are_distinct() {
        use std::collections::HashSet;
        let set: HashSet<u8> = [ExportKind::Named, ExportKind::Default, ExportKind::ReExport]
            .iter()
            .copied()
            .map(export_discriminant)
            .collect();
        assert_eq!(set.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Display
    // -----------------------------------------------------------------------

    #[test]
    fn display_is_hex_with_prefix() {
        let s = NodeId(0x1234_5678_9abc_def0).to_string();
        assert_eq!(s, "node-123456789abcdef0");
    }
}
