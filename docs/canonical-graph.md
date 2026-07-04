# Canonical graph

The canonical graph is cofferdam's typed representation of a project's
cross-file structure — files, symbols, imports, exports, layers, plus an
extension namespace for adapter-supplied facts. Cross-file checks query
it instead of joining flat corpus tables themselves; the predicate DSL
will compile graph queries against the same surface.

This page is the **adapter/check-author guide** to that surface. The
design rationale (decision matrix between Datalog backends, taxonomy
trade-offs, spike measurements) is in [ADR
0001](./decisions/0001-canonical-graph-schema.md). The versioning
contract sits inside the larger
[schema-versioning](./schema-versioning.md) policy.

## Lifecycle

The engine builds one canonical graph per analysis run, after pass 1
collects per-file `IMPORTS` and `EXPORTS` evidence, and writes it into a
corpus slot:

```rust
use cofferdam_graph::CANONICAL_GRAPH;

ctx.corpus.with_slot(&CANONICAL_GRAPH, |graph| {
    // graph is &Graph — query here in finalize().
});
```

Per-file `Check::run` cannot read the graph (it's populated AFTER pass 1
finishes). All graph-aware logic lives in `Check::finalize`. Today
`Design.OrphanExport` is the only built-in reading the slot;
`Design.LayerViolation`, `Design.ImportCycle`, and `Design.DeadExport`
migrate in follow-up beads.

## Closed core + namespaced extensions

The taxonomy is a **closed core** — every built-in node and edge is one
of a fixed set of Rust enum variants. Adapters extend the surface
through a single `Extension { ns, kind, attrs }` escape hatch. That gives
exhaustive `match` ergonomics on the built-in hot path and a bounded
output schema, while still letting `sql.*`, `iac.*`, `gql.*` adapters
add domain-specific facts.

### Nodes

| Variant | Identity | Notes |
|---|---|---|
| `File { path, lang }` | absolute, normalised path | Backslashes → `/`; lowercased on Windows. |
| `Symbol { name, kind }` | `(name, SymbolKind)` | `SymbolKind ∈ { Function, Class, Interface, Type, Variable, Namespace, Other }` |
| `Import { specifier }` | specifier string | Pre-resolution; rare in built-ins today. |
| `Export { name }` | export name | Reserved for future `DeadExport` work. |
| `Layer { name }` | layer name | Materialised by `assign_layer`. |
| `Extension { ns, kind, attrs }` | `(ns, kind, attrs)` | Adapter namespace + domain key. |

`NodeId` is a stable 64-bit content hash of the identifying payload —
two `add_node` calls with the same `NodeKind` return the same id and
don't duplicate the petgraph entry.

### Edges

| Variant | Direction | Notes |
|---|---|---|
| `DeclaredIn` | Symbol → File | Symbol's defining site. |
| `ImportsAsValue` | File → File | Value-position import. |
| `ImportsAsType` | File → File | `import type` or per-name `type:` import. |
| `ExportsAs { kind }` | File → Symbol/File | `ExportKind ∈ { Named, Default, ReExport }`. Not materialised yet (cp3 scope). |
| `BelongsToLayer` | File → Layer | Created by `Graph::assign_layer`. |
| `Extension { ns, kind, attrs }` | any | Adapter-defined relations. |

### Edge attributes

Each edge carries an `AttrMap` (`BTreeMap<SmolStr, Value>`,
JSON-roundtrippable). `Value` is `String | i64 | bool | List<Value>`.
The built-in import-edge attribute set:

| Key | Meaning |
|---|---|
| `import_kind` | `"default" \| "named" \| "namespace" \| "side_effect"` |
| `source_name` | Export name in the target module. `"default"` for default imports, `"*"` for namespace. |
| `local_name` | Binding name in the importing file. |
| `type_only` | Mirrors the `ImportsAsType` variant; lets consumers skip variant matching. |

Two edges between the same `(src, dst)` pair are kept distinct when
their attrs differ — that's how `import { foo, bar } from './m'`
produces two separate `ImportsAsValue` edges in one record.

## Querying

The `Graph` API exposes three query shapes:

### Direct neighbours

```rust
for (src, kind, attrs) in graph.incoming(file_node_id) {
    // src is the importing file's NodeId.
    // attrs lets you peek at import_kind / source_name / etc.
}

for (dst, kind, attrs) in graph.outgoing(file_node_id) {
    // dst is what this file imports / belongs to.
}
```

`incoming` is the workhorse for orphan-detection — walk a file's
incoming edges to summarise who consumes it.

### Path lookup

```rust
let normalised = cofferdam_graph::normalized_file_path(path);
let id = graph.node_id_for_path(&normalised);  // Option<NodeId>
```

The path index is populated automatically for every `NodeKind::File`
node. Always normalise paths before lookup — `normalized_file_path`
applies the same case-folding + slash-canonicalisation the engine uses
when building the graph.

### Bounded BFS / transitive closure

```rust
let reachable = graph.bfs_outgoing(start, 5, |kind| {
    matches!(kind, EdgeKind::ImportsAsValue | EdgeKind::ImportsAsType)
});

let depends_on_target = graph.reaches_via(from, target, 8, |_| true);
```

`bfs_outgoing` is depth-bounded and edge-filtered; visited nodes are
deduplicated. The depth cap is important: real codebases have cycles
and you don't want a worker walking a 500-node strongly-connected
component to answer a "does A transitively import B" predicate.

### Layer membership

```rust
for file_id in graph.files_in_layer("app") {
    // …
}
```

Layer membership is a denormalised index over `BelongsToLayer` edges —
direct map lookup, no traversal.

## Worked example: how OrphanExport queries the graph

`Design.OrphanExport` is the cleanest cross-file built-in and the first
consumer of the graph slot. The migration in cd-9hp.9 cp3 replaced its
`for imp in imports / for exp in exports` flat-table join with a
per-file incoming-edge summary:

```rust
fn summarise_incoming(g: &Graph, file_node: NodeId) -> FileConsumption {
    let mut out = FileConsumption::empty();
    for (_src, kind, attrs) in g.incoming(file_node) {
        if !matches!(kind, EdgeKind::ImportsAsValue | EdgeKind::ImportsAsType) {
            continue;
        }
        match attrs.get(&SmolStr::new_static("import_kind")) {
            Some(Value::String(s)) => match s.as_str() {
                "namespace" => out.ns_touched = true,
                "default" => out.default = true,
                "named" => {
                    let source_name = attrs.get(&SmolStr::new_static("source_name"));
                    if let Some(Value::String(n)) = source_name {
                        if n.as_str() == "default" {
                            out.default = true;          // `import { default as X }`
                        } else {
                            out.named.insert(n.clone());
                        }
                    }
                }
                "side_effect" => { /* consumes nothing */ }
                _ => {}
            },
            _ => {}
        }
    }
    out
}
```

The summary reduces to three pieces of state: a namespace flag, a
default flag, and a set of named-import claims. The per-export skip
logic then asks "is this export's name in the consumption set?" — no
graph traversal, just a hash lookup.

The four canonical-graph spec_contract fixtures pin every branch of
this reduction:

- `canonical-graph-type-only` — `ImportsAsType` counts as consumption.
- `canonical-graph-side-effect` — `side_effect` edges contribute
  nothing; an only-side-effect-imported file's named export stays
  orphan.
- `canonical-graph-namespace` — `ns_touched` shields named exports but
  not the default.
- `canonical-graph-default-rename` — Named edge with `source_name ==
  "default"` reroutes into the default bucket.

## Serialisation: the version envelope

When an adapter (cd-9hp.10) or the incremental cache (cd-9hp.4) needs
to emit or read a canonical graph as bytes, the wire shape is a JSON
object with a reserved top-level `schema_version`:

```json
{
  "schema_version": "1.0",
  "body": { /* adapter / cache payload */ }
}
```

`cofferdam_graph::load_envelope(bytes, path)` parses the envelope,
validates the version against this build's `(CURRENT_SCHEMA_VERSION,
MIN_SUPPORTED_SCHEMA_VERSION)` window, and returns:

```rust
pub struct GraphEnvelope {
    pub schema_version: SchemaVersion,
    pub schema_version_deprecated: bool,  // true → emit a hint
    pub body: serde_json::Map<String, serde_json::Value>,
}
```

Versions outside the window are rejected via `LoadError`
(`FutureSchemaVersion`, `UnsupportedSchemaVersion`,
`MalformedSchemaVersion`, `MissingSchemaVersion`, …) — every variant
is fatal. The future-version message names this cofferdam build via
`env!("CARGO_PKG_VERSION")` so users know which release to target.

The `body` is opaque to `cofferdam-graph` itself — its shape is owned
by the consumer (adapter contract or incremental cache). cp5 ships the
envelope so the version-checking contract is locked in before its
payload consumers ship, mirroring the technique
[cd-9hp.12](./schema-versioning.md) used for
`cofferdam.invariants.toml`.

## Authoring checklist for a graph-aware check

When you write the next cross-file check:

1. **Place all graph work in `finalize`.** Per-file `run` runs before
   the engine populates `CANONICAL_GRAPH`.
2. **Normalise file paths** before calling `node_id_for_path` —
   always via `cofferdam_graph::normalized_file_path`.
3. **Match `EdgeKind` variants exhaustively** where business rules
   depend on edge type. Filter early in BFS callbacks to skip
   `Extension` edges if you only care about built-ins.
4. **Read attrs by string key**, defensively. The attr set on a given
   edge type is documented above; if a key is missing, treat the edge
   as "no claim" rather than panicking.
5. **Depth-cap any closure query.** `reaches_via` and `bfs_outgoing`
   take a `max_depth`; pick a number that bounds CPU on pathological
   repos (8–16 is a reasonable default for import closures).
6. **Pin behaviour with a spec_contract fixture** under
   `crates/cofferdam-engine/tests/spec_contract/<check>/`. The
   canonical-graph fixtures above are the model.

## Out of scope (today)

- `Export` nodes and `ExportsAs` edges aren't materialised at cp3 —
  OrphanExport walks INCOMING import edges instead. These land when
  `DeadExport` / `ImportCycle` migrate.
- Adapter contract — domain adapters (SQL migrations, IaC manifests,
  GraphQL schemas) wire into the graph via the `Extension` escape
  hatch. The contract for declaring namespaces and emitting facts is
  cd-9hp.10's deliverable.
- ~~Incremental analysis — per-file provenance on each node/edge will
  give a natural invalidation unit for cd-9hp.4. The graph API stays
  build-only at cp2; mutation/removal lands with that bead.~~ Shipped
  in cd-36: `Graph::add_node_owned`/`add_edge_owned` tag every
  contribution with the normalised file path that produced it, and
  `Graph::remove_file(path)` drops exactly that file's contributions
  (a shared node survives until its last owner is removed). Wiring
  this into the engine's incremental replay flow is CD-32's scope, not
  this one's.
