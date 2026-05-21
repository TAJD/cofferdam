# ADR 0001 — Canonical graph schema (cd-9hp.9)

**Status:** Draft — Decision 1 (storage) and Decision 2 (taxonomy) accepted;
spike validates storage choice. Full schema rollout pending cd-9hp.9 proper.

**Context bead:** cd-9hp.9 (parent), cd-acv3 / cd-9hp.9.1 (storage spike).

## Why this decision matters

The corpus today is a flat namespace of typed slots; the project's
import/export graph lives implicitly across two well-known slots
(`IMPORTS`, `EXPORTS`). That shape works for current built-ins but
can't support transitive-closure predicates, edge-typed traversal,
domain-extension namespaces (`sql.*`, `iac.*`), or the incremental
analysis pipeline.

This ADR pins the substrate. The schema is the harder-to-migrate
call — the storage backend is comparatively easy to swap if the
graph API stays stable. The decisions here set the shape every
future adapter and DSL grammar bump compiles against.

## Decision 1 — Storage backend

**Accepted: petgraph + indexed attribute maps.**

Alternatives considered: Crepe (in-process Datalog), Ascent, embedded
Soufflé, custom triple store. Each is documented in cd-9hp.9's
design notes with its trade-offs.

petgraph wins on:
- Battle-tested directed graph (years of use across the Rust
  ecosystem), no bus-factor concern.
- Idiomatic Rust API — exhaustive matches on built-in node/edge
  kinds, no FFI or codegen step.
- No build-time dependency growth (Crepe/Ascent macros aside, Soufflé
  brings C++).
- The query workloads cd-9hp.9 actually serves (transitive closure
  bounded by depth, attribute-indexed lookup) map cleanly onto
  petgraph's `Graph` + adjacency iteration.

petgraph loses on:
- Declarative rule expression. Datalog rules read more naturally for
  open-ended rule sets; ours is a closed compiled output from the
  predicate DSL, so we don't pay that cost yet.

If the predicate DSL grows enough that ad-hoc traversal stops
scaling, the graph API stays stable across backends — re-evaluation
is bounded.

### Spike validation (cd-acv3)

The spike (`crates/cofferdam-engine/examples/graph_spike.rs`) builds a
petgraph `DiGraph` from cofferdam's existing IMPORTS/EXPORTS slots and
measures it against the flat-table baseline.

Numbers from a release build on Windows (the dev box), median of 5
runs per measurement:

| Repo | Files | Imports | Exports | Graph build | Build cost vs analyze |
|---|---|---|---|---|---|
| bestefforttools | 186 TS | 649 | 512 | 572µs (747 nodes, 1161 edges) | < 0.5% of analyze (115ms) |
| gistreact | 29 TS | 61 | 32 | 57µs (78 nodes, 93 edges) | < 0.3% of analyze (26ms) |

Graph build is effectively free relative to the parse + analyze cost
already in the pipeline. Well under cd-9hp.9's "≤ 1.5× the flat-table
baseline" acceptance gate.

Query throughput (32 queries per measurement, median of 5 runs):

| Workload | bestefforttools | gistreact |
|---|---|---|
| **petgraph direct** | 271 ns/query | 250 ns/query |
| flat-table direct | 646 ns/query (2.4×) | 593 ns/query (2.4×) |
| **petgraph closure d=2** | 703 ns/query | 509 ns/query |
| flat-table closure d=2 | 65.3 µs/query (93×) | 6.6 µs/query (13×) |
| **petgraph closure d=4** | 890 ns/query | 500 ns/query |
| flat-table closure d=4 | 65.1 µs/query (73×) | 6.2 µs/query (12×) |
| **petgraph closure d=8** | 887 ns/query | 493 ns/query |
| flat-table closure d=8 | 68.3 µs/query (77×) | 6.3 µs/query (13×) |

petgraph closure queries are 70-100× faster than re-walking the flat
ImportRecord slice on every query on bestefforttools, and 12-13×
faster on gistreact. The gap widens with depth on the larger fixture,
matching what we'd expect from indexed adjacency vs linear scan.

**Spike caveat to record:** the hit counts diverge between the two
implementations because the spike's query semantics simplified —
petgraph's direct-edge predicate matches resolved-File paths via
substring, missing edges where the specifier is relative
(`./foo`) but the resolved path is absolute. The flat-table baseline
self-matches via `imp.source_specifier == target_specifier`. The
*timings* still capture build + traversal cost honestly; the
production graph schema (decision 2 below) will carry the original
specifier as an edge attribute so direct-edge matching is exact.

### Verdict

**petgraph holds.** Proceed with cd-9hp.9 as scoped. No need to
re-evaluate Crepe / Ascent / Soufflé at this time.

## Decision 2 — Schema taxonomy

**Accepted: closed core + namespaced extensions.**

Alternatives considered: open string-keyed taxonomy (Kythe-style).

Closed core gives us:
- Exhaustive `match` on built-in `NodeKind` / `EdgeKind` — the
  compiler catches missed cases when we add a variant.
- Bounded JSON / SARIF schema for the public output surface.
- Lower runtime cost than string-key dispatch on the hot path.

Closed core costs us:
- Adapters can't add new core variants without a cofferdam version
  bump. They use the `Extension { ns, kind, attrs }` escape hatch
  instead.

The escape hatch is the same compromise Code Property Graphs and
Glean's Angle make. Kythe is the open-string counter-example and
pays for it with looser tooling — we explicitly don't follow that
path.

### v1 core schema (proposed for cd-9hp.9 proper)

```rust
pub enum NodeKind {
    File { path: PathBuf, lang: SmolStr },
    Symbol { name: SmolStr, kind: SymbolKind },
    Import { specifier: SmolStr },
    Export { name: SmolStr },
    Layer { name: SmolStr },
    Extension { ns: SmolStr, kind: SmolStr, attrs: AttrMap },
}

pub enum SymbolKind {
    Function, Class, Interface, Type, Variable, Namespace, Other,
}

pub enum EdgeKind {
    DeclaredIn,        // Symbol → File
    ImportsAsValue,    // File → File (via Import)
    ImportsAsType,     // File → File
    ExportsAs { kind: ExportKind },  // File → Symbol or File → File (re-export)
    BelongsToLayer,    // File → Layer
    Extension { ns: SmolStr, kind: SmolStr, attrs: AttrMap },
}

pub enum ExportKind { Named, Default, ReExport }
```

`AttrMap = BTreeMap<SmolStr, Value>` where
`Value = String | i64 | bool | List<Value>` — JSON-roundtrippable.

**Identity:** nodes get a stable `NodeId = u64` from a `HashBuilder`
hash of `(NodeKind discriminant, identifying attrs)`. Edges are
addressed by `(source_id, target_id, kind discriminant,
kind-specific attrs)` — multiple edges between the same two nodes
are allowed when their kind differs.

### What this unblocks

- **cd-9hp.1 v2** — DSL compiles graph queries against this surface.
  The v1 grammar reserves the syntactic room (`transitively imports`,
  edge-typed traversal, `core.*` / `ts.*` namespaces).
- **cd-9hp.10** — adapter contract. Each adapter declares its own
  `ns` and emits `Extension` nodes/edges; built-ins stay closed.
- **cd-9hp.4** — incremental analysis. Per-file provenance on every
  node/edge gives natural invalidation units.

### Schema versioning

Inherits cd-9hp.12's `schema_version` policy (MAJOR.MINOR, accepted
as integer or string, deprecation window, future-version refusal).
The graph's `schema_version` lives alongside the existing
`cofferdam.invariants.toml` version field; rule files declare the
DSL grammar version which references the graph schema version
transitively.

## Sequencing after this ADR

cd-9hp.9 proper picks up:

1. Core type definitions in `cofferdam-graph` (new crate) or
   `cofferdam-core::graph` (extension of the existing module). ~2
   days.
2. petgraph backend + attribute indexing. ~2-3 days.
3. Migrate `Design.OrphanExport` from flat-table joins to graph
   queries. ~1-2 days.
4. Benchmark + regression analysis. ~1 day.
5. `schema_version` wiring. ~1 day.
6. spec_contract fixtures + docs. ~1-2 days.

Total: 10-15 days, matching the bead's original estimate. The
spike's verdict is the green light to proceed.

## Out of scope for this ADR

- Predicate DSL v2 grammar (cd-9hp.1 v2 — separate bead).
- Adapter contract (cd-9hp.10 — separate bead, blocked on cd-9hp.9
  proper).
- Incremental analysis (cd-9hp.4 — separate bead).
- Migration of the second-cleanest cross-file check
  (`Design.LayerViolation` / `Design.ImportCycle` /
  `Design.DeadExport`) — done in follow-up beads as the graph API
  stabilises.
