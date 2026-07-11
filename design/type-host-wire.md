# Type-host wire protocol (cd-9hp.2)

The **type host** is a Node-side worker that exposes TypeScript's type system
to the Rust engine over a stdin/stdout JSON-RPC channel. Built-in checks
declaring `CheckMeta::requires_types = true` are routed through it instead of
the Rust pipeline.

This document is the contract between `crates/cofferdam-cli/src/type_host.rs`
(Rust client) and `crates/cofferdam-cli/scripts/type-host.mjs` (Node worker).

## Why a separate host

cofferdam's TypeScript surface uses [oxc](https://oxc.rs) for parsing —
fast, native Rust, but no type system. Type-aware checks (unused null
guards, narrowed-type misuse, branded-type leaks) need TS Compiler API
output, which lives in Node.

Decision summary (full discussion in cd-9hp.2):
- **Backend**: ts-morph (TS Compiler API wrapper) — mature, one library
  surface, ~200MB RAM, real type info.
- **Transport**: stdin/stdout newline-delimited JSON-RPC. Same pattern as
  the existing plugin host (cd-81a.7), battle-tested, excellent crash
  containment (child dies, parent gets EOF, respawn).
- **v1 scope**: built-in checks only. Plugins declaring `requiresTypes:
  true` are warned but not routed (cd-9hp.2.B reopens this when a real
  plugin use case appears).

## Resolution of `ts-morph`

The Node worker uses bare-specifier `import("ts-morph")`. To find the
package without bundling it into cofferdam itself, the Rust client spawns
Node with `NODE_PATH=<project-root>/node_modules`, so resolution falls
back to the project's own `node_modules` after the standard ESM lookup
fails. Projects that want type-aware checks must `npm install ts-morph`
(or have it as a transitive dep) in the project root.

Cofferdam ships **without** a bundled ts-morph. A clear error response
surfaces when ts-morph isn't resolvable; the CLI maps it to a single
warning and skips type-aware checks rather than failing the run. A
fallback "install on first use" path is not implemented — projects opt
in by installing ts-morph themselves (see `docs/type-aware-checks.md`).

## Wire framing

Both directions use **newline-delimited JSON** (NDJSON):

- One JSON object per `\n`-terminated line on stdin (Rust → Node).
- One JSON object per `\n`-terminated line on stdout (Node → Rust).
- Stderr is free-form diagnostics, never read by the parser.

The Node worker stays alive until stdin closes (EOF), then flushes any
in-flight responses and exits with code 0. On unrecoverable error the
worker exits non-zero with a diagnostic on stderr.

## Request shape

```json
{ "id": "<correlation-id>", "method": "<name>", "params": { ... } }
```

- `id`: client-chosen string, echoed in the matching response. Used to
  pair responses when multiple requests are in flight. Required.
- `method`: one of the named methods below.
- `params`: method-specific object, may be omitted when empty.

## Response shape

Success:

```json
{ "id": "<correlation-id>", "ok": true, "result": { ... } }
```

Error:

```json
{
  "id": "<correlation-id>",
  "ok": false,
  "error": { "code": "<short-code>", "message": "<human-readable>" }
}
```

Error codes (extend as new methods land):
- `ts_morph_unavailable` — `import("ts-morph")` failed; result is `null`.
  The Rust client treats this as fatal for the type-host run; built-in
  checks declaring `requires_types` are skipped and a `Warning.TypeHost*`
  finding surfaces.
- `project_init_failed` — `Project` constructor threw (bad tsconfig
  path, permissions, malformed compiler options).
- `method_unknown` — the requested `method` is not implemented by this
  host version. The Rust client treats this as a version mismatch.
- `internal` — uncaught exception inside the host; `message` carries
  the JS error string.

## Methods

### `ping` (cp1)

A diagnostic / cold-start measurement method. Available in every host
version; downstream methods may piggyback its timings via the same
shape.

**Request `params`:**

```json
{
  "loadTsMorph": true,
  "openProject": { "tsconfigPath": "/abs/path/to/tsconfig.json" }
}
```

- `loadTsMorph` (bool, default `true`): if `true`, dynamic-import
  `ts-morph` and record the elapsed milliseconds. If `false`, skip the
  import — useful for measuring pure Node spawn cost.
- `openProject` (object, optional): if present and `loadTsMorph` is
  `true`, also construct a ts-morph `Project` rooted at `tsconfigPath`
  and record that timing. The project handle is discarded after timing
  (cp1 doesn't persist any state between requests).

**Response `result`:**

```json
{
  "tsMorphVersion": "21.0.1",
  "timings": {
    "tsMorphImportMs": 1234,
    "projectInitMs": 287,
    "totalMs": 1521
  }
}
```

- `tsMorphVersion` (string|null): the loaded ts-morph package's
  `package.json#version`, or `null` if loading was skipped or failed.
- `timings.tsMorphImportMs` (u64|null): wall-clock ms from request
  receipt to `import("ts-morph")` resolution. `null` when
  `loadTsMorph: false`.
- `timings.projectInitMs` (u64|null): wall-clock ms to construct the
  `Project`. `null` when `openProject` was omitted or import failed.
- `timings.totalMs` (u64): wall-clock ms from request receipt to
  response emission.

### `openProject` (cp2)

Open (or return the cached) ts-morph `Project` for a tsconfig and force
eager source-file resolution, so the per-query path hits a warm
project. The worker caches the `Project` keyed by tsconfig path for the
rest of its lifetime — the CLI calls this once up front so the multi-
second init isn't a mysterious mid-run stall.

**Request `params`:** `{ "tsconfigPath": "/abs/tsconfig.json" }`

**Response `result`:**

```json
{ "sourceFileCount": 312, "initMs": 2515, "cached": false }
```

- `sourceFileCount` (u64): files the project resolved from the tsconfig.
- `initMs` (u64): wall-clock ms the init took; `0` when `cached: true`.
- `cached` (bool): `true` when the project was already open in the
  worker (a no-op repeat call).

### `typeAt` (cp2)

Resolve the type of the AST node spanning a byte range. The worker
translates the oxc UTF-8 byte offsets to the TS Compiler API's UTF-16
character positions (using the file text it already holds), finds the
node via `getDescendantAtStartWithWidth` (falling back to
`getDescendantAtPos`), and reports compact type facts.

**Request `params`:**

```json
{
  "tsconfigPath": "/abs/tsconfig.json",
  "file": "/abs/src/foo.ts",
  "startByte": 6,
  "endByte": 7
}
```

The worker lazy-opens the project if `openProject` wasn't called first.
File-path matching tolerates slash/drive-case differences and will
`addSourceFileAtPathIfExists` a file the tsconfig globs missed.

**Response `result`:** `TypeFacts`, or JSON `null` when no meaningful
type could be resolved (file not in project, no node at span, node has
no type). The Rust client maps a `null` result to `None`, which checks
treat as "can't conclude — emit nothing".

```json
{
  "text": "string | null",
  "isNullable": true,
  "includesNull": true,
  "includesUndefined": false,
  "isAny": false
}
```

- `text` (string): the type's printed form. Human-facing only.
- `isNullable` (bool): type includes `null` or `undefined`.
- `includesNull` / `includesUndefined` (bool): per-constituent flags,
  computed over the type's union members.
- `isAny` (bool): type is `any` or `unknown`. Narrowing checks MUST
  bail on this — the compiler can't prove a guard redundant against a
  type it knows nothing about.

### `resolveLiteral` (CD-82)

Resolve an identifier/import reference to a literal value. Follows
ts-morph symbol resolution — an imported binding's symbol is aliased to
the symbol at its origin declaration — so `import { x } from
"./constants"` resolves through to `const x = "..."` in `constants.ts`
as long as both files are part of the open Project (the whole tsconfig's
file set is already loaded, so this "just works" without extra wiring).

Same byte-range-to-node resolution as `typeAt`: UTF-8 byte offsets are
translated to UTF-16 positions, the node is found via
`getDescendantAtStartWithWidth` (falling back to `getDescendantAtPos`).

**Request `params`:**

```json
{
  "tsconfigPath": "/abs/tsconfig.json",
  "file": "/abs/src/page.ts",
  "startByte": 9,
  "endByte": 20
}
```

Same shape as `typeAt`'s params — `startByte`/`endByte` should span the
identifier reference (e.g. the imported binding's use site, or its
declaration name).

**Response `result`:** `LiteralFacts`, or JSON `null` when nothing is
resolvable at all (the node at that span isn't an identifier, has no
symbol, or the symbol has no declarations). The Rust client maps a
`null` result to `None`.

```json
{
  "literalString": "A great page about widgets",
  "isNullable": false,
  "isEmptyObject": false
}
```

- `literalString` (string, optional): present only when the resolved
  declaration's initializer is a string literal or no-substitution
  template literal.
- `isNullable` (bool): the declared/inferred type includes `null` or
  `undefined`, or the initializer is a `null`/`undefined` literal.
- `isEmptyObject` (bool): the initializer is an object literal with zero
  properties (`{}`).

A resolved declaration whose initializer isn't a literal (a function
call, a non-empty object, etc.) still returns `LiteralFacts` with
`literalString` absent rather than `null` — mirrors `typeAt`'s
"best-effort facts, `null` only when truly nothing resolvable"
philosophy. `null` is reserved for "couldn't even identify a symbol to
resolve", not "resolved to something uninteresting".

No new error codes — failures to open the project or resolve ts-morph
surface via the same `ts_morph_unavailable` / `project_init_failed`
codes `typeAt` uses.

## Worker pool (CD-31)

The Rust client no longer holds a single worker. `build_type_oracle` spawns a
**pool of N Node worker processes**, each an independent instance of this
wire protocol — same request/response shapes, same methods, no framing
change. N defaults to the host's available parallelism (`pool_size()` in
`type_host.rs`), overridable via `COFFERDAM_TYPE_HOST_POOL_SIZE`. Each
worker opens the same tsconfig independently (its own in-process ts-morph
`Project` cache — workers do not share memory), so pool startup cost is
roughly N × one worker's `openProject` cost; workers are opened concurrently
(one thread per worker) so wall-clock cost stays close to a single worker's
init time rather than N times it.

`WorkerTypeOracle::type_at` dispatches to a worker round-robin via an atomic
counter. This is what keeps type-aware check throughput from serialising
back onto one process once the per-file engine loop is parallelized
(CD-30) — each concurrent caller gets a (probabilistically) different
worker instead of blocking on a single mutex. A pool of size 1 (forced via
`COFFERDAM_TYPE_HOST_POOL_SIZE=1`, or the default on a single-core host)
behaves identically to the pre-CD-31 single-worker design.

Graceful degradation is unchanged: if any worker in the pool fails to spawn
or fails `openProject` (Node missing, ts-morph not installed, bad
tsconfig), `build_type_oracle` tears down every worker it already started
and returns the error — the CLI's existing single-clear-diagnostic /
no-type-aware-checks fallback applies to the whole pool, not per-worker.

## Future methods

Sketched for forward-compatibility; not yet implemented.

### `resolveTypes` (batch)

Given a file and a list of node selectors, return facts for all of them
in one round-trip. The engine would batch a check's per-file queries so
the NDJSON channel isn't hit once per node. Deferred until a real check
shows the per-query round-trip is a bottleneck — `typeAt` is correct
and simple in the meantime.

### `shutdown`

Explicit shutdown request. The host flushes pending responses and exits
0. Without this, closing stdin has the same effect (the cp1/cp2 client
relies on EOF).

## Measured cold-start (cp1 baseline)

Captured on a Windows 11 host (Node 22.20.0, ts-morph 28.0.0) via
`cofferdam type-host --ping`. All numbers are wall-clock ms from request
receipt to the relevant boundary.

| Workload | Files | ts-morph import | Project init | Total |
|---|---|---|---|---|
| Pure spawn (`--no-load`) | — | n/a | n/a | ~0 |
| Load ts-morph only | — | 680 (cold) / 190 (warm) | n/a | 680 / 190 |
| Load + Project init, gistreact tsconfig | 31 | 191 | 2515 | 2706 |
| Load + Project init, bestefforttools tsconfig | 325 | 195 | 12127 | 12322 |

Implications for cp2+:
- The 12-second Project-init cost on a 325-file project means the
  worker MUST be reused across the engine's per-file dispatch, not
  spawned per check or per file. The cp2 engine routing plans a single
  long-lived worker per analysis run.
- The warm-import number (~190ms) reflects Windows file-system caching;
  the first run of the day pays the cold cost (~680ms).
- Project init scales roughly linearly with file count. A 10k-file
  workspace would project ~400s of project-init cost — at that point
  ts-morph's `useInMemoryFileSystem` + selective `addSourceFileAtPath`
  becomes worth investigating.

These are intentionally captured as a baseline rather than a tight
regression gate. The cp4 CI smoke test (`.github/workflows/type-host-smoke.yml`,
driven by `scripts/check-type-host-smoke.mjs`) pins a *generous*
max-acceptable Project-init duration against the committed fixture
project `examples-type-host/unused-null` — it catches a hang or a 10x
regression without flaking on slow runners, rather than asserting a
precise number.

## Versioning

The wire is implicitly v1 for cp1's `ping` shape. When a request adds a
required field or changes a response shape, bump the wire version by
adding a `wireVersion` field to every request and response and
documenting the bump here. The cd-9hp.12 schema-versioning policy
applies — additive changes are minor, structural changes are major.
