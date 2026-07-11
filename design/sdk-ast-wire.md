# Plugin host AST wire format — design spike

> Status: **draft, awaiting review.** Not in the published docs site.
> Created: 2026-05-05. Authors: TAJD + Claude.
> Bead: cd-4de. Blocks cd-b5h (NoHttpClient / Pattern B) + cd-11j (TenantIsolation / Pattern C).
> Related: `design/sdk-ast-surface.md` (the v0 type surface this serialises) and `design/platform-extensibility.md` (the language-adapter split this lives within).

## Why this exists

cd-7e4 landed `cofferdam check` → Node plugin host → JSON merge. Today the
host ships `SourceFile` with `ast: null`. That is sufficient for line-walk
plugins (BrandCasing) but blocks every plugin that wants to query the AST —
which is the next two beads on the board (cd-b5h, cd-11j) and the entire
Pattern B / Pattern C class of checks.

Deciding the wire format is now a real architectural commitment: the shape
plugins receive at runtime is part of the public contract from the moment
the first AST-querying plugin ships. cd-7e4 picked subprocess+JSON as the
process model; this spike picks how AST data flows across that boundary.

## What plugins need (from `design/sdk-ast-surface.md`)

The v0 SDK surface (8 node kinds, frozen in cd-717) gives plugins three
operations against `file.ast`:

1. **`view.findAll<K>(kind: K): readonly NodeOfKind<K>[]`** — eager filter
   in document order. Used by Pattern B (NoHttpClient queries
   `findAll("ImportDeclaration")` and `findAll("CallExpression")`).
2. **`view.walk(visitor: AstVisitor): void`** — depth-first traversal,
   per-kind callbacks return `Walk.Continue` (descend) or `Walk.Skip`
   (don't descend, keep going at sibling level). Used by Pattern C
   (TenantIsolation accumulates state across a stateful walk).
3. **`view.root: AstNode`** — escape-hatch root reference for surgical
   inspection. Pattern C touches this when the typed surface doesn't cover
   a case. Rarely used in practice; not on the critical path.

Spans on every node must round-trip: bytes sliced by `(node.span.start_byte,
node.span.end_byte)` from the original source must equal the literal node
text. `cofferdam fix` autofix payloads depend on this.

## Cost reference (for grounding the options)

Validated corpus stats (`C:/Users/tajdi/bestefforttools`, our largest test
target):

| Metric | Value |
|---|---|
| Files | 155 |
| Total source | 778 KB / 25,972 lines |
| Largest single file | 2,235 lines, ~80 KB |
| Average file | 5.0 KB / 167 lines |
| Estimated AST nodes / file (avg) | ~500–1,000 |
| Estimated AST nodes / file (largest) | ~5,000–10,000 |

Per-node JSON serialisation (option D below):
- `kind` (15-char string) + `span` (4 u32s) + 2–4 typed fields + 2 child
  offset u32s ≈ **80–150 bytes**.

Estimated manifest size:
- Average file: ~1,000 nodes × 100 B = **~100 KB** per-file manifest.
- 155-file run: **~15 MB** of stdin pipe traffic + Node-side allocation.
- Largest single file: **~500 KB–1 MB** in one shot.

These are upper bounds — most plugins touch only 2–3 node kinds. We're
serialising the full AST regardless; that's the cost of letting `findAll`
and `walk` run synchronously on the host without a roundtrip per query.

## Process model (settled, not re-litigated here)

cd-7e4 chose Rust CLI → spawn `node plugin-host.mjs` → JSON over stdin/stdout.
This spike does **not** revisit that decision. Alternatives like in-process
QuickJS, embedding Node into the Rust binary, or shipping a napi addon as
the host runtime are deliberately out of scope — they're a separate
process-model spike if any of them ever becomes the right call.

The constraint, then, is: AST data must round-trip as JSON-serialisable
values flowing across stdin/stdout, with one full-file manifest per
`cofferdam check` invocation.

## The four real options

### Option A — Eager flat array (no descent control)

Rust walks the AST once, emits `nodes: AstNode[]` in document order. Host
builds `Map<NodeKind, AstNode[]>` for `findAll`; `walk` iterates the array
linearly.

```json
{
  "nodes": [
    { "kind": "ImportDeclaration", "span": {...}, "source": "axios" },
    { "kind": "CallExpression", "span": {...}, "calleeIdx": 0 }
  ]
}
```

| Pro | Con |
|---|---|
| Simplest wire format. | `walk()`'s `Walk.Skip` semantics don't work — the host can't descend or skip-subtree without parent/child structure. |
| `findAll` is a single linear pass per kind. | Plugin Pattern C (the whole point of `walk`) is unsupported. |

Verdict: **rejected**. Half of the SDK surface stops working.

### Option B — Per-kind roundtrip (lazy queries)

Host stays AST-less. Each `findAll(kind)` round-trips back to Rust over
stdout. `walk` shipped as a single roundtrip that streams visit events.

| Pro | Con |
|---|---|
| Zero AST data sent unless the plugin asks. | Subprocess IPC is uni-directional today (host writes JSON, exits). Going bidirectional means a long-running message protocol. |
| Plugins paying only for what they touch. | Per-query latency: TenantIsolation walks the whole tree — `walk` becomes one massive query that's no smaller than option A or D. |
| | Fundamentally redesigns the process model — a separate spike. |

Verdict: **rejected**. Forces a bidirectional protocol; we're keeping the
process model as decided in cd-7e4.

### Option C — Hierarchical nested JSON

Mirror the typescript-eslint / babel-eslint shape: every node has its
typed fields plus a `children: AstNode[]` (or per-shape named children
like `body`, `arguments`).

```json
{
  "kind": "Program",
  "span": {...},
  "body": [
    { "kind": "ImportDeclaration", "span": {...}, "source": "axios", "specifiers": [...] },
    {
      "kind": "ExpressionStatement",
      "expression": {
        "kind": "CallExpression",
        "callee": { "kind": "MemberExpression", ... },
        "arguments": [...]
      }
    }
  ]
}
```

| Pro | Con |
|---|---|
| `walk` is natural — recursive descent matches the structure. | `findAll(kind)` still requires a full traversal per call (or a one-time index pass). |
| Familiar to plugin authors who know ESTree. | Larger payload (every parent encodes children inline). |
| Tree shape is recoverable for future SDK growth. | Building child-shape per node kind is a maintenance burden — add a node, update the serialiser shape. |
| | Many "non-relevant" expressions (numeric literals inside arguments, etc.) still ship even though our v0 surface doesn't expose them. |

Verdict: **viable but not preferred**. Familiar shape, but pays for tree
structure plugins don't use much in practice (Pattern B does pure
`findAll`; Pattern C touches `walk` but only at top-level kinds).

### Option D — Flat array with child-offset table (RECOMMENDED)

Same flat array as A, but each node carries `firstChildIdx` /
`nextSiblingIdx` (or equivalently `childRangeStart` / `childRangeEnd`) so
the host can implement descent control without the nested tree.

```json
{
  "nodes": [
    { "kind": "Program", "span": {...}, "firstChild": 1, "nextSibling": -1 },
    { "kind": "ImportDeclaration", "span": {...}, "source": "axios",
      "firstChild": 2, "nextSibling": 5 },
    { "kind": "ImportSpecifier", "span": {...}, "firstChild": -1, "nextSibling": 3 },
    ...
  ],
  "rootIdx": 0
}
```

Host implements `findAll(kind)` as a single linear filter; `walk(visitor)`
as a recursive descent over the indices, honouring `Walk.Skip` by jumping
to `nextSibling` instead of descending into `firstChild`.

| Pro | Con |
|---|---|
| Wire format stays flat (cheap to JSON-encode and JSON.parse). | Child-offsets must be computed Rust-side during the walk that builds the array — small extra book-keeping. |
| `findAll` is O(N) one-pass; `walk` honours `Walk.Skip` correctly. | Plugin authors never see the indices (those stay private to the host) — but they're part of the wire contract, so any wire-format changes are breaking. |
| Adding a new kind is one new entry in the nodes array, no shape change. | Slight payload bloat from `-1` sentinel offsets. |
| Memory layout matches how `view.find_all(NodeKind)` already works in `cofferdam-ts::ast::AstView`. | |

Verdict: **recommended.** Best fit for our subprocess+JSON constraint and
the v0 surface's mix of `findAll`+`walk`.

### Option E — Hybrid: ship LineView + flat AST + lazy heavy fields (FUTURE)

Variant of D that defers expensive payloads: per node, ship `kind` + `span`
+ `firstChild`/`nextSibling` always, but lazy-load specific fields
(`ImportDeclaration.source`, `MemberExpression.property`) via a separate
side table the host pulls keyed by node index.

| Pro | Con |
|---|---|
| Manifest skeleton is small (~30 B/node); heavy strings only paid for nodes that get them. | Two-pass wire format. JSON doesn't naturally support "value by reference"; needs a second array indexed by node. |
| Zero allocation for the long tail of nodes plugins never query. | Worth doing only when the validated corpus shows a measurable problem. |

Verdict: **defer**. Premature without measurement. File as a follow-on
optimization bead if option D's manifest size becomes a real bottleneck.

## Recommendation: ship option D in v0

**Rationale (in priority order):**

1. **Both Pattern B and Pattern C work without IPC redesign.** `findAll`
   is a linear filter; `walk` is a recursive descent that honours `Skip`.
   This is the minimum viable shape for cd-b5h *and* cd-11j.
2. **Wire format stays cheap to encode and parse.** No nested arrays of
   typed shapes per node kind; just a flat list with two extra integer
   fields.
3. **Adding a new node kind is one entry in the array, not a shape
   change.** Aligns with cd-717's "additive growth" philosophy on the
   public type surface.
4. **Manifest size is bounded.** ~15 MB upper-bound for a 155-file
   bestefforttools run is large but tolerable for v0; option E sits behind
   it as a clean optimisation path if the bound becomes a problem.
5. **Familiar fallback.** If a plugin author asks "where's the parent
   pointer?", we add `parentIdx` to each entry without breaking the wire
   format. Hierarchical (option C) doesn't have an obvious extension path
   without doubling the encoded structure.

## Wire format spec (proposed, additive growth allowed)

```ts
interface AstWire {
  /** Index into `nodes` of the Program root. Always 0 in practice but
   *  carried explicitly so we don't bake the assumption into plugin code. */
  rootIdx: number;
  nodes: WireNode[];
}

/** Discriminated by `kind`. Common fields appear on every node;
 *  per-kind typed fields layer on top. */
type WireNode =
  | WireProgram
  | WireCallExpression
  | WireImportDeclaration
  | WireFunction
  | WireArrowFunctionExpression
  | WireClass
  | WireObjectExpression
  | WireMemberExpression
  | WireIdentifierReference;

interface WireNodeBase<K> {
  kind: K;
  /** Byte-offset span. Round-trips against the original source. */
  span: { line: number; column: number; start_byte: number; end_byte: number };
  /** Index into `nodes` of the first child, or -1 if none. */
  firstChild: number;
  /** Index into `nodes` of the next sibling, or -1 if none. */
  nextSibling: number;
}
```

Per-kind extensions match `design/sdk-ast-surface.md`:

| Kind | Wire-only extra fields |
|---|---|
| `Program` | (none beyond base) |
| `CallExpression` | `calleeIdx: number`, `argumentIdxs: number[]` |
| `ImportDeclaration` | `source: string`, `specifiers: { localName: string; imported?: string }[]` |
| `Function` | `name?: string`, `paramIdxs: number[]`, `async: boolean`, `generator: boolean` |
| `ArrowFunctionExpression` | `paramIdxs: number[]`, `async: boolean`, `expression: boolean` |
| `Class` | `name?: string` |
| `ObjectExpression` | `propertyIdxs: number[]` |
| `MemberExpression` | `objectIdx: number`, `property?: string`, `computed: boolean` |
| `IdentifierReference` | `name: string` |

Wherever the public SDK type names another `AstNode` (e.g.
`CallExpression.callee: AstNode`), the wire form ships an integer index
into `nodes`. The host's `buildSourceFile` step rehydrates these into
typed object references when constructing the `AstView`'s `findAll` /
`walk` views, so plugin code sees the shape `design/sdk-ast-surface.md`
locked.

## Span fidelity

Every wire node MUST satisfy:

```
source.slice(node.span.start_byte, node.span.end_byte) === <node's literal source>
```

The CLI already invokes `parse_into` to build LineViews; emitting AST
nodes piggybacks on the same parse pass via a single `Visit` walk that
collects node entries with their `span: oxc_span::Span`. No re-parsing.

CI guardrail (proposed): an SDK-side test that takes a corpus fixture,
runs every node's `(start_byte, end_byte)` slice against the source, and
fails if any byte range doesn't match a known oxc node string form.

## Subtree-skip semantics

`Walk.Skip` jumps to the current node's `nextSibling`. The host's
`view.walk` is roughly:

```ts
function walk(view: AstView, visitor: AstVisitor) {
  const recurse = (idx: number) => {
    if (idx < 0) return;
    const node = view.nodes[idx];
    const cb = visitorMethod(visitor, node.kind);
    const decision = cb ? cb(node) : Walk.Continue;
    if (decision !== Walk.Skip && node.firstChild >= 0) recurse(node.firstChild);
    recurse(node.nextSibling);
  };
  recurse(view.rootIdx);
}
```

`firstChild === -1` means a leaf (or a node we don't expose children
through — e.g. `IdentifierReference.name` is a string, not an AstNode).
`nextSibling === -1` means end-of-siblings; the recurse falls through.

## Memory + timing budget (estimated, needs measurement)

For the bestefforttools 155-file run:

| Phase | Cost (estimated) |
|---|---|
| Rust-side AST flatten (one Visit pass per file) | ~10–20 ms total (oxc parse already does the heavy lifting) |
| JSON encode (serde_json) | ~50–100 ms total |
| stdin pipe traffic | ~15 MB streamed |
| Node-side `JSON.parse` | ~80–150 ms total (depends on V8 tuning) |
| Plugin run() time | depends on plugin |

These are upper-bound projections — actual measurement happens when
option D ships. The numbers are within the ~270 ms full-corpus baseline
recorded in CLAUDE.md ("bestefforttools | 325 files | 269 ms"); any
20%+ regression after this lands triggers an optimisation pass (option
E becomes a real bead at that point).

## What lands in cd-b5h vs what lands here

Splitting the work to keep cd-b5h focused on the fixture, not the wire
format:

**This spike (cd-4de) — design only.** No code. Output is this doc + a
companion bead for the implementation work below.

**New bead "AST wire format implementation"** — to be filed:
- Rust-side: extend `crates/cofferdam-cli/src/plugins.rs::ManifestFile`
  with `ast: Option<AstWire>`. Add a Visit-based collector that builds
  the flat-array + child-offset table. ~150–200 lines.
- JS-side: extend `crates/cofferdam-cli/scripts/plugin-host.mjs`'s
  `buildSourceFile` to construct `AstView` from the wire payload. Build
  per-kind index for `findAll`, recursive walker for `walk`. ~100 lines.
- SDK-side: no public-API changes (the `design/sdk-ast-surface.md` types
  already exist). Possibly tighten the SDK's `runPlugin` to assert
  `ast !== null` for `findAll`/`walk`-using plugins.
- Tests: span round-trip + per-fixture corpus.

**cd-b5h (NoHttpClient)** — fixture only. Plugin source, fixture file,
expected.json. Zero infra changes.

**cd-11j (TenantIsolation)** — fixture only. Same scope.

## Open questions — resolutions

1. ~~**Should we ship `parentIdx` on every wire node?**~~
   **Resolved 2026-05-05: defer.** No named fixture (BrandCasing,
   NoHttpClient, TenantIsolation) needs ancestor traversal — TenantIsolation
   accumulates state across a stateful walk, and the visitor's call stack
   already provides ancestor context. Adding `parentIdx` later is
   non-breaking (host code that doesn't read it ignores it; a future wire
   rev can add the field without invalidating older plugins). Applies the
   cd-717 discipline literally: no fixture, no v0. The "plugin authors
   will eventually want it" argument is real but lacks a forcing
   function — files as a follow-on bead the moment a real plugin demands
   it.

2. ~~**Should `firstChild` index ALL children regardless of SDK kind?**~~
   **Resolved 2026-05-05: index every oxc node.** Wire emits actual oxc
   `kind` strings (`BinaryExpression`, `NumericLiteral`, etc.) for nodes
   outside the v0 surface, but the SDK's typed `AstNode` union only
   covers the v0 subset. Runtime values for non-v0 kinds appear as
   `AstNode` shapes whose `kind` doesn't match any v0 variant; TypeScript
   narrows them away in `findAll<K>` and `walk(visitor)` (no visit method
   is called for an unsurfaced kind). Plugin authors writing manual
   walks from `view.root` see all kinds — that's the documented escape
   hatch. The cost (~5× larger manifest vs filtering) is real but
   bounded; the structural-fidelity preserves wire stability across
   future v0 surface additions and avoids a class of "callee points at
   something we filtered out" footguns. Compression / lazy field tables
   (option E) sit behind this as a clean optimisation path if measurement
   shows a problem.

3. ~~**What about the SDK's `view.root: AstNode`?**~~
   **Resolved 2026-05-05: no special handling.** `view.root === nodes[rootIdx]`.
   Pairs with resolution #4 — `root.kind === "Program"` once Program is
   in the union.

4. ~~**Do we need a `kind: "Program"` entry in the node enum?**~~
   **Resolved 2026-05-05: yes.** Necessary for typing `view.root: AstNode`
   soundly — without `Program` in the union, root narrows to `never` on
   any TypeScript switch and authors hit a wall the first time they
   touch the escape hatch. One-line addition to
   `design/sdk-ast-surface.md`'s NodeKind union with shape
   `{ kind: "Program"; span: Span; body: readonly AstNode[] }`. Updates
   v0 surface count to **9 kinds** (5 strict + 4 borderline) where
   Program slots in alongside Function/ArrowFunctionExpression/Class
   under "structural anchors."

5. ~~**Compression?**~~
   **Resolved 2026-05-05: defer.** Ship uncompressed in v0; file a
   follow-on bead when measurement shows pipe-write or `JSON.parse`
   dominating runtime. The 15 MB upper-bound figure is for the full
   bestefforttools corpus; typical use is far smaller. Premature without
   numbers from a real run.

## What "done" looks like

For this spike (cd-4de):
- This doc lands on `main` and is reviewed in a doc-only PR.
- Open questions 1–4 are resolved (in-doc) before the implementation
  bead is filed.
- A follow-on bead "AST wire format implementation" is created with
  acceptance criteria mirroring this doc's wire format spec.

For the implementation bead (separate, follow-up):
- Rust + JS sides ship the wire format.
- Span round-trip CI guardrail in place.
- One AST-using fixture (cd-b5h or cd-11j, whichever lands first)
  passes end-to-end with `cofferdam check`.

## Decision

**Option D (flat array + child-offset table).** Open questions resolved
above; final v0 wire shape:

- Per-node fields: `kind`, `span`, `firstChild`, `nextSibling`, plus
  per-kind typed extras. **No `parentIdx`** in v0 (deferred — Q1).
- Every oxc node indexed in `nodes`, including kinds outside the v0
  surface (Q2). The wire's `kind` field carries the actual oxc kind
  name; the SDK's typed `AstNode` union narrows to the v0 subset.
- `view.root === nodes[rootIdx]` (Q3); `Program` is in the v0
  `NodeKind` union (Q4) so root is typeable.
- Uncompressed v0; option E (lazy field tables) deferred until
  measurement (Q5).

Reviewable as a doc PR. Implementation lands as a separate bead once
this is approved.

## Transport streaming (CD-33, wireVersion 2)

The per-node `AstWire` shape above (kind/span/firstChild/nextSibling +
per-kind extras) is unchanged. What changed is *how many manifests* cross
the stdin/stdout boundary per `cofferdam check` run: originally one, now
one NDJSON record per file.

**Before (v1, one-shot):** the Rust client serialised every file's
`{path, text, lineViews, layer, ast}` into a single `PluginManifest`
JSON object, wrote it once to the child's stdin, and read one JSON
object (`{reports, errors}`) back from stdout after EOF. Peak memory on
both sides was O(repo) — the "cost reference" table above (~15 MB for
the bestefforttools corpus) is the size of that one blob.

**After (v2, streamed):** both directions are NDJSON, matching the
type-host's framing (`design/type-host-wire.md`). Stdin carries:

```json
{"type":"header","wireVersion":2,"cwd":"...","plugins":[...],"options":{...},"tsconfigPath":"..."}
{"type":"file","path":"...","text":"...","lineViews":[...],"layer":null,"ast":{...}}
... (one "file" record per source file, in order) ...
{"type":"end"}
```

`tsconfigPath` (CD-81) is `null`/absent when no tsconfig was discovered or
type-awareness is disabled; when present, it's the same tsconfig the built-in
type oracle would use, and lets `plugin-host.mjs` resolve types in-process via
ts-morph (`crates/cofferdam-cli/scripts/type-host-core.mjs`, shared with the
standalone type-host worker in `design/type-host-wire.md`) for any loaded
check declaring `requiresTypes: true`.

Stdout carries, streamed as each file is processed:

```json
{"type":"report","checkId":"...","message":"...","file":"...","startByte":0,"endByte":0,"severity":"..."}
{"type":"error","kind":"load_failed"|"run_threw"|"finalize_threw","plugin":"...","file":"...","message":"..."}
{"type":"done","typeHostUnavailable":null}
```

`typeHostUnavailable` (CD-81) is present on the `done` record only when at
least one loaded check declared `requiresTypes: true`: `null` when ts-morph
resolved fine, or a human-readable reason string when it couldn't (no
tsconfig, ts-morph not installed). `plugins.rs` surfaces a non-null reason as
a synthetic `Warning.PluginTypeHostUnavailable` finding so it flows through
baselining/suppression/`--fail-on` like any other finding, and so
`--fail-on-type-unavailable` can gate on it the same way it gates on the
built-in type oracle's availability.

Peak memory is now O(one file) on both sides instead of O(repo) — each
`ManifestFile` (with its `AstWire`) is built, written, and dropped
before the next file starts (`crates/cofferdam-cli/src/plugins.rs`).

**Deadlock avoidance.** Streaming means Node may start writing `report`/
`error` records before Rust has finished writing all `file` records —
a naive single-threaded write-then-read loop risks the classic
bidirectional-pipe deadlock if stdout fills the OS pipe buffer while
the writer side is still blocked on stdin. The Rust client dedicates a
background thread to draining stdout (and a second for stderr)
concurrently with the main thread's writes, joining both after the
`try_wait()` timeout loop determines the child has exited or been
killed.

**Ordering + finding parity.** The Node host (`plugin-host.mjs`)
processes `file` records strictly in arrival order via a serialized
async chain (each line's handler must finish — including any
in-flight plugin dynamic `import()`s from a `header` record — before
the next line is handled), preserving the original file-outer/
plugin-inner report ordering. This is the exact-parity gate: every
`examples-plugins/*/expected.json` golden must match byte-for-byte
against the streamed path (verified via `scripts/check-plugin-fixtures.mjs`).

**Unaffected paths.** `query_plugin_metadata`'s one-shot
`{"mode":"metadata",...}` request (small, no per-file data) keeps its
original one-shot request/response shape — streaming buys nothing
there. The `COFFERDAM_PLUGIN_HOST_DUMP_WIRE` debug dump (consumed by
`scripts/check-ast-spans.mjs`) still writes one combined
`[{path, text, ast}]` array at `end` time, accumulated across `file`
records — this re-introduces O(repo) memory deliberately, but only
when the debug env var is set.

**Versioning.** `WIRE_VERSION: u32 = 2` in `plugins.rs` marks this as a
structural (major) change per the cd-9hp.12 schema-versioning policy —
the framing changed even though the AST node payload didn't.

**Completion handshake (cd-41).** `{"type":"done"}` is not just the
loop-exit signal for the Rust reader — it's the only thing that
distinguishes "the host legitimately found nothing" from "the host's
output was lost." On a constrained machine running several plugin
hosts concurrently, `process.exit()` called immediately after a stdout
write can outrun the OS pipe flush (observed on Windows, where piped
stdout writes are asynchronous) and truncate output, including the
final `done` line — the child still exits 0. `plugin-host.mjs` now
sets `process.exitCode` and lets the event loop drain naturally instead
of forcing exit, so the buffered write actually completes first. As
defense in depth, `read_stream_records` in `plugins.rs` also tracks
whether `done` was actually observed; if the child exits successfully
but the stream ends without it, that's surfaced as
`Warning.PluginHostFailed` rather than treated as an empty finding
set — see `crates/cofferdam-cli/tests/plugin_findings.rs`'s
`host_exit_without_completion_marker_surfaces_as_plugin_host_failed`.
