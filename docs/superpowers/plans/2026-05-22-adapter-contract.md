# Adapter Contract (cd-9hp.10) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Status:** DRAFT for review. No beads created yet, no code written. The "Design decisions" section below has open forks the controller must ratify before sub-bead 10a is executed.

**Goal:** Turn the canonical-graph ingestion path into a declared, one-way extension point — an `Adapter` trait whose only output is canonical-graph facts — and refactor the existing TypeScript pipeline to be the first adapter rather than a hardcoded special case.

**Architecture:** Today the engine hardcodes TS extraction (`graph::GraphBuilder` → `IMPORTS`/`EXPORTS` corpus slots → `build_canonical_graph` → `CANONICAL_GRAPH` slot) and branches on `file.language` in its per-file loop. We introduce an `Adapter` trait (source artifacts → graph facts, *nothing else*), register adapters by the language tag they claim, and make the engine dispatch to adapters instead of branching inline. TS becomes `TsAdapter`; the spec-contract fixture suite is the "behaviour unchanged" gate. The first non-TS proof-of-life adapter (SQL migrations) and the adapter SDK guide are deferred to their own sub-beads, exactly as the bead body requests.

**Tech Stack:** Rust workspace (`cofferdam-core`, `cofferdam-graph`, `cofferdam-engine`, `cofferdam-rust`, new `cofferdam-adapter-sql`). oxc (TS parse), tree-sitter (Rust parse). Canonical graph schema in `cofferdam-graph`. Validation via `cargo test -p cofferdam-engine spec_contract`.

---

## Where we are today (grounding)

Read before touching anything:

- **Canonical schema** — `crates/cofferdam-graph/src/schema.rs`. `NodeKind` / `EdgeKind` are a closed core (`File`, `Symbol`, `Import`, `Export`, `Layer`) plus `Extension { ns, kind, attrs }` for adapter-defined types. Node identity is content-addressed (`crates/cofferdam-graph/src/id.rs::compute_node_id`).
- **TS extraction** — `crates/cofferdam-engine/src/graph.rs`. `GraphBuilder::collect(file, parsed, corpus)` walks the oxc AST and appends TS-shaped `ImportRecord`/`ExportRecord` (defined in `cofferdam-core::graph`) into the `IMPORTS`/`EXPORTS` corpus slots.
- **Graph build** — `crates/cofferdam-graph/src/build.rs::build_canonical_graph(imports, exports) -> Graph`. Called once in `crates/cofferdam-engine/src/lib.rs:676` after pass 1, writes the `CANONICAL_GRAPH` slot. Only `Design.OrphanExport` reads the canonical graph today; every other graph-aware check still joins the flat `IMPORTS`/`EXPORTS` slots directly.
- **Per-language dispatch** — `crates/cofferdam-engine/src/lib.rs:440-560`. The per-file loop branches on `file.language` (`Language` enum in `crates/cofferdam-core/src/source.rs`). Rust files go through `parse_rust`; checks declare `Check::language()` (`crates/cofferdam-core/src/check.rs:333`) and are filtered per file. TS is the fall-through.
- **Rust adapter** — `crates/cofferdam-rust/src/lib.rs`. Explicitly "Phase 0 — pre-canonical-graph": it only emits `Issue`s via the `Check` trait. It produces **no** graph facts.
- **Config** — `crates/cofferdam-engine/src/config.rs`. `[plugins]` exists (`plugins: Vec<PathBuf>`, config.rs:84); there is no `[adapters]` key.
- **No `Adapter` trait, no `GraphDelta` type exists anywhere in the workspace.** This is greenfield abstraction over existing concrete code.
- **Contract spec** — `docs/design-principles.md` §4 (lines 222-276). This is the authoritative description of the two contracts (§4.1), the seam that breaks genericity (§4.2), the shared-rule test (§4.3), and the three leaks — Span/Identity/Taxonomy (§4.4).

**Gap assessment:** This is "formalize + refactor what exists," not "build from scratch." The schema, the graph store, and per-language dispatch already exist. What's missing is the trait boundary, the registration mechanism, and the discipline that the ingestion side cannot reach the rule side.

---

## Design decisions (RATIFY BEFORE EXECUTING)

These are the forks that shape every sub-bead. Recommendation first, then the alternative and why I didn't pick it. Push back on any of these in review.

### D1 — Adapter output is a graph delta, applied by the engine

`Adapter` produces canonical-graph nodes + edges (a `GraphDelta`), and the engine merges them into the run's `Graph`. It does **not** hand back TS-shaped `ImportRecord`/`ExportRecord` (those are a TS-internal intermediate). The contract output type is `cofferdam_graph::{NodeKind, EdgeKind}`.

- *Why:* This is verbatim §4.1 ("Output: typed nodes and edges in the canonical graph"). It's the only output shape that generalizes to SQL/IaC/GraphQL.
- *Alternative rejected:* Keep the flat-table-slot indirection as the contract. Rejected because `ImportRecord` is TS-shaped — a SQL adapter has no imports. The flat tables survive only as an **internal** detail of `TsAdapter` for backward compat (see D5).

### D2 — In-process Rust trait now; external/JS adapter variant deferred

Define a synchronous in-process Rust trait. Both shipped adapters (TS, and later SQL) are in-process Rust. The "external-process / JS plugin" adapter variant the bead mentions is deferred to a later sub-bead.

- *Why:* The contract's genericity is proven by a second *in-process* language (SQL), not by an IPC boundary. External adapters add an IPC protocol, a handshake, and a versioning surface that aren't needed to satisfy the "is the contract real?" test, and the user's standing preference is to defer the speculative forward-compat tail until the core is validated.
- *Alternative rejected:* Design the trait IPC-first from day one. Rejected as premature — we have zero external-adapter consumers, and an in-process trait can grow an external bridge later without rework.

### D3 — The one-way contract is enforced *by construction* in-process; the runtime load-guard is deferred with external adapters

The strongest enforcement is the trait signature itself: `Adapter` is handed a file and a graph sink, and returns graph facts. It is given **no** `&mut Vec<Issue>`, **no** rule config, **no** corpus handle. An in-process adapter therefore *cannot compile* code that emits an `Issue` or reads rule config. The "deliberate-violation fixture" becomes a `trybuild` compile-fail test proving the trait grants no Issue-emitting capability.

- *Why:* Compile-time impossibility beats a runtime check. §4.1's "an adapter that emits a finding has bypassed the rule layer" is unrepresentable rather than merely detected.
- **Honest caveat for review:** The bead's acceptance says "Engine *refuses to load* an adapter that tries to emit Issues." A *runtime* load-time refusal is only meaningful for an *external* adapter that can send arbitrary bytes over a wire. For in-process Rust adapters the guard is moot (the type system already forbids it). So I recommend: satisfy this acceptance criterion in-process via the `trybuild` proof, and move the *runtime* load-guard into the deferred external-adapter sub-bead where it actually has teeth. **This is a reinterpretation of the literal acceptance text — flag for your sign-off.**

### D4 — This bead formalizes `adapter → graph-facts`. It does NOT sever `rules → AST`.

The §4.2 seam — rule authors peeking at oxc/tree-sitter AST nodes via `ctx.parsed` — stays open after this bead. TS checks legitimately read the oxc AST today; closing that seam is a multi-quarter migration of every built-in onto graph queries and is explicitly out of scope here.

- *Why:* Bundling the rule-side migration into this bead explodes scope from "one refactor + one new adapter" to "rewrite every check." The bead's own acceptance is about the *adapter* side and one proof-of-life adapter — not about rules.
- *Plan impact:* `docs/adapter-contract.md` documents both contracts (§4.1) but states plainly that the rule→graph migration is tracked elsewhere and the AST seam is a known, accepted, temporary leak (§4.2).

### D5 — `TsAdapter` keeps populating `IMPORTS`/`EXPORTS` internally during the transition

Because only `Design.OrphanExport` reads the canonical graph and every other graph-aware check still joins the flat slots, `TsAdapter` must continue to populate `IMPORTS`/`EXPORTS` **and** emit canonical-graph deltas, until those checks migrate. The flat slots are an internal implementation detail of `TsAdapter`, not part of the `Adapter` contract surface.

- *Why:* This is what makes "behaviour unchanged on the existing fixture suite" achievable. Removing the flat slots in this bead would require migrating ~half a dozen checks first — out of scope (see D4).
- *Alternative rejected:* Cut over to canonical-graph-only now. Rejected — it forces the rule-side migration this bead explicitly defers.

### D6 — The `Adapter` trait lives in `cofferdam-graph`

`cofferdam-graph` already depends on `cofferdam-core` (for `SourceFile`/`Language`/`Span`) and owns the `NodeKind`/`EdgeKind`/`GraphDelta` types an adapter emits. Putting the trait there avoids a new crate and a dependency-cycle risk.

- *Alternative rejected:* A new `cofferdam-adapter` crate. Rejected as premature crate-proliferation; revisit only if the external-adapter bridge needs its own home.

---

## Scope boundary (what this bead does and does not do)

**In scope (across sub-beads 10a–10e):**
- An `Adapter` trait + `GraphDelta` type (10a).
- `docs/adapter-contract.md` (10a).
- TS extraction refactored to `TsAdapter` implementing the trait, behaviour unchanged (10b).
- Adapter registration + `[adapters]` config + compile-fail proof of the one-way contract (10c).
- One non-TS adapter (SQL migrations) end-to-end as proof-of-life (10d).
- Adapter SDK guide (10e).

**Explicitly out of scope (tracked elsewhere / deferred):**
- Severing rules from source ASTs (the §4.2 seam) — D4.
- External-process / JS adapter variant + its runtime load-guard — D2/D3.
- Migrating non-OrphanExport checks off the flat `IMPORTS`/`EXPORTS` slots — D5.
- Per-adapter *finding* identity schemes (§4.4) — findings come from rules, not adapters; the node-identity story (`compute_node_id`) is already solved.

---

## Sub-bead breakdown + sequencing

Proposed children of `cd-9hp.10` (the epic parent stays `cd-9hp`). **Not yet created** — see "Handoff."

```
cd-9hp.10 (this bead — becomes the umbrella/contract owner)
├── 10a  Adapter trait + GraphDelta + docs/adapter-contract.md   [no behaviour change]
│        └─ blocks → 10b, 10c
├── 10b  Refactor TS extraction into TsAdapter (behaviour unchanged, spec_contract gate)
│        └─ depends on 10a; blocks → 10d
├── 10c  Adapter registry + [adapters] config + trybuild contract proof
│        └─ depends on 10a
├── 10d  SQL-migrations proof-of-life adapter (cofferdam-adapter-sql)   [the "is it real?" test]
│        └─ depends on 10b, 10c
└── 10e  Adapter SDK guide (docs/adapter-sdk-guide.md)
         └─ depends on 10d (write the guide once the path is walked end-to-end)
```

Sequencing rationale: 10a locks the design everything else compiles against — it must land first and is the safest (doc + trait, no behaviour change). 10b and 10c are parallelizable after 10a (different files: 10b touches `graph.rs`/engine loop, 10c touches `config.rs` + a new registry module) — but both touch the engine's central orchestration, which CLAUDE.md flags as **not safe to parallelize**, so run them sequentially, 10b then 10c. 10d is the payoff and needs both. 10e documents the walked path.

**This plan details 10a fully.** 10b–10e are sketched (their detailed task plans get written once 10a's design is ratified, since they compile against the trait 10a defines).

---

## Sub-bead 10a — Adapter trait + GraphDelta + contract doc

**Files:**
- Create: `crates/cofferdam-graph/src/adapter.rs`
- Modify: `crates/cofferdam-graph/src/lib.rs` (add `pub mod adapter;` + re-exports)
- Create: `docs/adapter-contract.md`
- Test: unit tests inside `crates/cofferdam-graph/src/adapter.rs`

No engine changes in 10a — the trait is defined and unit-tested in isolation. Wiring happens in 10b.

### Task 1: `GraphDelta` accumulator type

A `GraphDelta` is what an adapter emits: a batch of nodes and edges to merge into the run graph. It is the adapter's *only* output channel.

- [ ] **Step 1: Write the failing test**

In `crates/cofferdam-graph/src/adapter.rs` (new file), at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{EdgeKind, NodeKind, SymbolKind};
    use smol_str::SmolStr;
    use std::path::PathBuf;

    #[test]
    fn delta_records_nodes_and_edges() {
        let mut d = GraphDelta::new();
        let f = d.add_node(NodeKind::File {
            path: PathBuf::from("/p/a.ts"),
            lang: SmolStr::new_static("typescript"),
        });
        let s = d.add_node(NodeKind::Symbol {
            name: SmolStr::new("Widget"),
            kind: SymbolKind::Class,
        });
        d.add_edge(s, f, EdgeKind::DeclaredIn, AttrMap::new());
        assert_eq!(d.nodes().len(), 2);
        assert_eq!(d.edges().len(), 1);
    }

    #[test]
    fn delta_dedupes_identical_nodes_by_id() {
        let mut d = GraphDelta::new();
        let a = d.add_node(NodeKind::Import { specifier: SmolStr::new("react") });
        let b = d.add_node(NodeKind::Import { specifier: SmolStr::new("react") });
        assert_eq!(a, b, "identical payload must share a NodeId");
        assert_eq!(d.nodes().len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cofferdam-graph adapter::tests -- --nocapture`
Expected: FAIL — `GraphDelta` is not defined.

- [ ] **Step 3: Write minimal implementation**

At the top of `crates/cofferdam-graph/src/adapter.rs`:

```rust
//! The adapter contract: source artifacts → canonical-graph facts.
//!
//! An [`Adapter`] is the ONLY place language- or format-specific
//! ingestion lives. Its sole output is a [`GraphDelta`] — a batch of
//! canonical-graph nodes and edges. By construction it has no channel
//! to emit `Issue`s, read rule configuration, or call user code: the
//! trait signature simply doesn't hand it those capabilities. See
//! `docs/adapter-contract.md` and `docs/design-principles.md` §4.

use std::collections::HashMap;

use crate::id::compute_node_id;
use crate::schema::{EdgeKind, NodeKind};
use crate::value::AttrMap;
use crate::NodeId;

/// A pending edge: endpoints by `NodeId`, kind, and attribute payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEdge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub attrs: AttrMap,
}

/// The adapter's output: nodes + edges to merge into the run graph.
///
/// Nodes are de-duplicated by content-addressed [`NodeId`] so an
/// adapter can naively `add_node` the same `File` from many call sites
/// without inflating the graph.
#[derive(Debug, Clone, Default)]
pub struct GraphDelta {
    nodes: HashMap<NodeId, NodeKind>,
    edges: Vec<DeltaEdge>,
}

impl GraphDelta {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a node; returns its stable content-addressed id. Calling
    /// twice with identical payload returns the same id and stores once.
    pub fn add_node(&mut self, node: NodeKind) -> NodeId {
        let id = compute_node_id(&node);
        self.nodes.entry(id).or_insert(node);
        id
    }

    pub fn add_edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind, attrs: AttrMap) {
        self.edges.push(DeltaEdge { from, to, kind, attrs });
    }

    pub fn nodes(&self) -> &HashMap<NodeId, NodeKind> {
        &self.nodes
    }

    pub fn edges(&self) -> &[DeltaEdge] {
        &self.edges
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }
}
```

> NOTE for implementer: confirm `compute_node_id`'s exact signature in `crates/cofferdam-graph/src/id.rs` (the explore pass saw `compute_node_id(&NodeKind) -> NodeId` re-exported from `lib.rs`). If it takes a borrow of a different shape, adjust the call but keep the dedupe semantics. Also confirm `AttrMap` is re-exported from the crate root (`lib.rs:68` shows it is).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cofferdam-graph adapter::tests`
Expected: PASS (both tests).

- [ ] **Step 5: Wire the module + re-exports**

In `crates/cofferdam-graph/src/lib.rs`, add to the `pub mod` block (after `pub mod build;`):

```rust
pub mod adapter;
```

and to the re-export block (after the `build::` re-export line):

```rust
pub use adapter::{Adapter, AdapterMeta, DeltaEdge, GraphDelta};
```

(`Adapter`/`AdapterMeta` don't exist yet — Task 2 adds them; this line will not compile until then. Land Steps 5 of Task 1 and Task 2 together in one commit, OR temporarily re-export only `GraphDelta`/`DeltaEdge` here and widen in Task 2. Prefer the latter to keep each commit compiling.)

Temporary form for this commit:

```rust
pub use adapter::{DeltaEdge, GraphDelta};
```

- [ ] **Step 6: Verify + commit**

Run: `cargo build -p cofferdam-graph && cargo test -p cofferdam-graph`
Expected: PASS.

```bash
git add crates/cofferdam-graph/src/adapter.rs crates/cofferdam-graph/src/lib.rs
git commit -m "feat(graph): add GraphDelta adapter-output accumulator (cd-9hp.10a)"
```

### Task 2: The `Adapter` trait + `AdapterMeta`

The trait that says "source artifacts in, graph facts out, nothing else." This is the load-bearing contract.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/cofferdam-graph/src/adapter.rs`:

```rust
struct NoopAdapter;

impl Adapter for NoopAdapter {
    fn meta(&self) -> &'static AdapterMeta {
        const M: AdapterMeta = AdapterMeta {
            id: "test.noop",
            language: "typescript",
            globs: &["*.noop"],
            namespace: None,
        };
        &M
    }
    fn analyze(&self, _file: &cofferdam_core::SourceFile) -> GraphDelta {
        let mut d = GraphDelta::new();
        d.add_node(NodeKind::Import { specifier: SmolStr::new("x") });
        d
    }
}

#[test]
fn adapter_analyze_returns_a_delta() {
    let a = NoopAdapter;
    let file = cofferdam_core::SourceFile::new(
        std::path::PathBuf::from("/p/a.noop"),
        String::new(),
    );
    let delta = a.analyze(&file);
    assert_eq!(delta.nodes().len(), 1);
}

#[test]
fn adapter_meta_declares_globs_and_namespace() {
    let a = NoopAdapter;
    assert_eq!(a.meta().id, "test.noop");
    assert_eq!(a.meta().globs, &["*.noop"]);
    assert!(a.meta().namespace.is_none(), "core-only adapter declares no extension namespace");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cofferdam-graph adapter::tests`
Expected: FAIL — `Adapter` / `AdapterMeta` not defined.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/cofferdam-graph/src/adapter.rs` (above the `tests` module):

```rust
use cofferdam_core::SourceFile;

/// Declarative, `&'static` metadata describing one adapter. Mirrors the
/// role `CheckMeta` plays for checks: the single source of truth the
/// engine reads to register and dispatch the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterMeta {
    /// Stable, dotted identity (`ts.core`, `sql.migrations`). Don't rename.
    pub id: &'static str,
    /// The `Language` tag (matching `cofferdam_core::Language::id_str`)
    /// whose files this adapter ingests.
    pub language: &'static str,
    /// File globs this adapter claims (`*.ts`, `*.sql`, ...). Declared
    /// up-front; the engine routes files to adapters by this set.
    pub globs: &'static [&'static str],
    /// The schema-extension namespace this adapter emits `Extension`
    /// nodes/edges under (`Some("sql")`, `Some("iac")`). `None` for an
    /// adapter that emits only closed-core nodes (the TS adapter).
    pub namespace: Option<&'static str>,
}

/// The one-way ingestion contract: source artifacts → canonical-graph
/// facts.
///
/// **By construction this trait grants no other capability.** `analyze`
/// receives a read-only [`SourceFile`] and returns a [`GraphDelta`]. It
/// is handed no `Issue` sink, no rule configuration, and no corpus. An
/// adapter therefore *cannot* bypass the rule layer — the type system
/// forbids it. This is the in-process realisation of `design-principles`
/// §4.1's adapter contract; the runtime load-guard for external/JS
/// adapters is tracked separately (see `docs/adapter-contract.md`).
pub trait Adapter: Send + Sync {
    fn meta(&self) -> &'static AdapterMeta;

    /// Parse `file` and emit its canonical-graph facts. The adapter owns
    /// its own parsing (oxc, tree-sitter, a SQL lexer, ...). Pure over
    /// the file's `(path, text)` — no global state, no I/O beyond what
    /// resolution requires.
    fn analyze(&self, file: &SourceFile) -> GraphDelta;
}
```

> NOTE for implementer: this initial `analyze(&self, file) -> GraphDelta` deliberately omits a pre-parsed handle and any resolver. The TS adapter needs an `oxc_resolver` for import resolution and wants to reuse the engine's single parse — that signature widening (e.g. an `AdapterContext` carrying a resolver + the already-parsed view, still with NO issue/config channel) is **a 10b design step**, not 10a. Keeping 10a's signature minimal avoids guessing the engine-side shape before we wire it. Document this in the contract doc as "signature may gain a capability-scoped context in 10b; it will never gain an Issue or rule-config channel."

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cofferdam-graph adapter::tests`
Expected: PASS (all four tests).

- [ ] **Step 5: Widen the re-export**

In `crates/cofferdam-graph/src/lib.rs`, change the temporary re-export from Task 1 to:

```rust
pub use adapter::{Adapter, AdapterMeta, DeltaEdge, GraphDelta};
```

- [ ] **Step 6: Verify + commit**

Run: `cargo build --workspace && cargo test -p cofferdam-graph && cargo clippy -p cofferdam-graph --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: PASS / clean.

```bash
git add crates/cofferdam-graph/src/adapter.rs crates/cofferdam-graph/src/lib.rs
git commit -m "feat(graph): add Adapter trait + AdapterMeta (cd-9hp.10a)"
```

### Task 3: `docs/adapter-contract.md`

The acceptance criterion is an explicit contract doc. It must cover input, output, allowed/forbidden APIs, schema-extension rules, and the identity-scheme story.

- [ ] **Step 1: Write the doc**

Create `docs/adapter-contract.md` with these sections (write real prose, not headers-only):

1. **Purpose** — one paragraph: adapters are the only place language/format-specific ingestion lives; their sole output is canonical-graph facts. Link `design-principles.md` §4.
2. **Input** — a `SourceFile` (path + text). The adapter owns parsing. (Note the planned 10b capability-scoped context for resolution; state it will never carry an Issue/config channel.)
3. **Output** — a `GraphDelta` of `NodeKind`/`EdgeKind`. Closed-core kinds for concepts the built-ins already reason about (`File`, `Symbol`, `Import`, `Export`); `Extension { ns, kind, attrs }` for domain-specific facts.
4. **Allowed** — declare claimed file globs; declare one extension namespace (`AdapterMeta.namespace`); emit closed-core + own-namespace `Extension` nodes/edges.
5. **Forbidden (and how it's enforced)** — no emitting `Issue`s, no reading rule config, no calling user code. State the enforcement is *by construction* in-process (the trait grants no such capability; a `trybuild` compile-fail test in 10c pins it) and that the runtime load-guard for external adapters is future work.
6. **Schema-extension rules** — namespaces are declared up-front in `AdapterMeta`; built-in code treats `(ns, kind)` opaquely (cite `schema.rs` doc comments); cross-domain rules match by string, per §4.3 the shared-rule test.
7. **Identity** — node identity is content-addressed via `compute_node_id` (already solved). Per-adapter *finding* identity is a rule-layer concern, not an adapter one; cross-reference §4.4 and state it's out of scope for the adapter contract.
8. **Known temporary leak** — per D4/§4.2, rules can still read source ASTs via `ctx.parsed`; closing that seam is tracked separately. Be explicit so a reader doesn't think the contract is already airtight.
9. **Writing a new adapter** — a short pointer stub (the full SDK guide is 10e): implement `Adapter`, declare `AdapterMeta`, register in `[adapters]` (10c), model on `TsAdapter` (10b) or `cofferdam-adapter-sql` (10d).

- [ ] **Step 2: Check it renders / no broken intra-doc links**

If the repo runs `cofferdam gen-docs --check` in the pre-commit hook (CLAUDE.md says it does), confirm this new page doesn't need a nav/index entry that the check enforces. Run:

Run: `cargo run -p cofferdam-cli -- gen-docs --check` (or whatever the hook invokes — inspect `.githooks/pre-commit`)
Expected: PASS, or a clear instruction to add a nav entry (add it if so).

- [ ] **Step 3: Commit**

```bash
git add docs/adapter-contract.md
git commit -m "docs: adapter contract — input/output/allowed/forbidden (cd-9hp.10a)"
```

### Task 4: Close-out verification for 10a

- [ ] **Step 1: Full verification block** (from CLAUDE.md)

Run:
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Expected: all green. 10a adds a trait + type + doc with no engine wiring, so the existing suite (including `spec_contract`) must be untouched — if any existing test changed, something leaked that shouldn't have.

- [ ] **Step 2: Confirm no behaviour change**

Run: `cargo test -p cofferdam-engine spec_contract`
Expected: PASS with zero `expected.json` diffs. 10a touches no engine code; a diff here means a mistake.

---

## Sub-bead 10b — Refactor TS extraction into `TsAdapter` (SKETCH)

Detailed task plan to be written after D1/D6 are ratified. Shape:

- Introduce a capability-scoped `AdapterContext` (carries the shared `oxc_resolver` + the already-parsed `ParsedView`; carries NO issue sink, NO rule config) and widen `Adapter::analyze` to take it. This is the signature decision deferred from 10a Task 2.
- Move `GraphBuilder::collect`'s walk into a `TsAdapter` that (a) still appends `IMPORTS`/`EXPORTS` slots (D5 backward-compat) and (b) emits a `GraphDelta` of `File` + import edges equivalent to what `build_canonical_graph` produces today.
- Replace the engine's hardcoded `build_canonical_graph(&imports, &exports)` call (`lib.rs:676`) and the inline per-language branch (`lib.rs:440-560`) with adapter dispatch: look up the adapter for `file.language`, call `analyze`, merge the delta into the run `Graph`.
- **Gate:** `cargo test -p cofferdam-engine spec_contract` must pass with zero `expected.json` diffs. That is the literal "behaviour unchanged on the existing fixture suite" acceptance criterion. The canonical-graph `build.rs` unit tests must also still pass (or move into `TsAdapter`).
- Risk: the engine's parse-once-reuse model. `AdapterContext` must let same-language checks keep reading `ctx.parsed` — the adapter must not force a re-parse. Verify against the real repos (`bestefforttools`, `gistreact`) per CLAUDE.md.

## Sub-bead 10c — Registry + `[adapters]` config + contract proof (SKETCH)

- Add an adapter registry (parallel to how `all_builtins()` registers checks): a function returning the built-in adapters, dispatched by `AdapterMeta.language`/`globs`.
- Add `[adapters]` to `ProjectConfig` in `config.rs`, mirroring `[plugins]` (config.rs:84). In-tree adapters are built-in; the config key reserves the surface for future external adapters (do NOT build external loading here — D2).
- **Contract proof:** a `trybuild` compile-fail test (new dev-dependency, or reuse if present) demonstrating that an `impl Adapter` body cannot construct/emit an `Issue` or read rule config — there's no parameter for it. This satisfies the "engine refuses an adapter that emits Issues" acceptance via compile-time impossibility (D3). Document the runtime-guard deferral in the test's header comment and in `adapter-contract.md`.
- Open question for review: is `trybuild` an acceptable new dev-dep, or does the controller prefer a hand-rolled `compile_fail` doctest? (Doctests can assert non-compilation via ```` ```compile_fail ````, no new dep — likely preferred.)

## Sub-bead 10d — SQL-migrations proof-of-life adapter (SKETCH)

- New crate `crates/cofferdam-adapter-sql` implementing `Adapter`: globs `*.sql`, namespace `"sql"`, emits `Extension { ns: "sql", kind: "table"/"column"/... }` nodes/edges from parsed migration files (a lightweight SQL statement splitter — full SQL parsing is not required for proof-of-life; tables + columns + the migration file node suffice).
- Add `Language::Sql` to `cofferdam-core/src/source.rs` + `Language::from_path` mapping.
- One end-to-end `spec_contract`-style fixture proving an SQL file flows source → adapter → canonical graph, and (stretch) one cross-domain rule that reads `export`-shaped edges from both TS and SQL to demonstrate §4.3's shared-rule test. If the shared-rule demo is too large, the proof-of-life is just "SQL facts land in the graph" and the shared rule becomes its own bead.
- This is the "is the contract real?" acceptance. Its own sub-bead per the bead body.

## Sub-bead 10e — Adapter SDK guide (SKETCH)

- `docs/adapter-sdk-guide.md`: how to write an adapter, what namespaces are allowed, how to test (model on the SQL adapter's fixture). Written last, after the path is walked end-to-end, so it documents reality rather than intention. Model on the existing `docs/plugin-sdk-guide.md`.

---

## Self-review

**Spec coverage** (against the five acceptance criteria in `bd show cd-9hp.10`):
1. "Adapter contract documented in `docs/adapter-contract.md`" → 10a Task 3. ✓
2. "TS pipeline refactored to implement the adapter trait; behaviour unchanged (spec_contract gate)" → 10b. ✓ (sketch; gate named explicitly)
3. "Engine refuses to load an adapter that emits Issues / reads rule config; deliberate-violation fixture" → 10c, reinterpreted as compile-time impossibility + `compile_fail` proof (D3). ⚠️ **Reinterpretation flagged for sign-off** — literal "load-time refusal" only bites for external adapters, which are deferred.
4. "One non-TS adapter shipped end-to-end as proof-of-life (SQL recommended)" → 10d. ✓ (its own sub-bead, as the bead body itself directs)
5. "Adapter SDK guide for plugin authors" → 10e. ✓

**Placeholder scan:** 10a (the executable part) has complete code in every step. 10b–10e are intentionally sketches, not executable steps — clearly labelled, because their signatures depend on 10a's design being ratified (writing fake detail now would be a placeholder of the worst kind). This is a deliberate plan structure, not a gap.

**Type consistency:** `GraphDelta` (Task 1) → consumed by `Adapter::analyze` return (Task 2) → re-exported in lib.rs (both tasks). `AdapterMeta` fields (`id`, `language`, `globs`, `namespace`) are referenced consistently in Task 2's test, the trait, and the 10c config sketch. `compute_node_id` / `AttrMap` / `NodeId` are all confirmed re-exported from `cofferdam-graph` lib.rs:59-68.

**Open forks to ratify before executing 10a:** D3 (compile-time vs runtime guard — reinterprets acceptance #3) and D2/D4 (deferring external adapters + the rule→AST migration). D1/D5/D6 are lower-risk but listed for completeness.
