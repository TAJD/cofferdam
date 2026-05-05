# `@cofferdam/check-sdk` — v0 AST surface freeze

> Status: **draft, awaiting review.** Not in the published docs site.
> Created: 2026-05-04. Authors: TAJD + Claude.
> Source: cd-717 (sub-design of cd-81a.2). Parent: `design/platform-extensibility.md` "Work to file" bullet 3.

## Why this exists

cd-81a.2 (the AST surface for plugins) is the highest-leverage decision in the
plugin epic. Once a plugin is written against the v0 node types, those types
are public API forever — every plugin in the wild has to recompile if the
shape moves.

The SDK on `cd-7e4-plugin-sdk-e2e` already ships an AST surface
(`packages/check-sdk/src/ast.ts`). This doc is the freeze: it enumerates every
type, field, visitor method, and ctx helper that crosses the public boundary
in v0, and gates each on a concrete fixture pattern. **Anything without a
justifying fixture stays out of v0** — additive growth happens later, on
demand, when a real plugin needs it.

The doc is reviewable as a PR before any v0 node renames or additions ship.

## The fixtures driving v0

Three e2e fixtures in `examples-plugins/` exercise the three plugin patterns
and define the surface we owe plugin authors:

| Fixture | Bead | Pattern | What it touches |
|---|---|---|---|
| `brand-casing` | cd-7e4 | A — line walk + magic comments | `file.lines()`, `LineView` flags, `lineView.spanFor()` |
| `no-http-client` | cd-b5h | B — eager AST `findAll` | `ImportDeclaration`, `CallExpression`, `MemberExpression`, `IdentifierReference` |
| `tenant-isolation` | cd-11j | C — stateful walk with file decision | `ImportDeclaration`, `CallExpression`, `MemberExpression`, `ObjectExpression`, `IdentifierReference` |

Every entry below is justified against this set, plus a small handful of
"defensible" entries that round out the discriminated union without expanding
plugin reach (see [§ Borderline](#borderline-entries)).

## Decisions

### Decision 1 — surface shape: interfaces with discriminated union

**Status: reaffirmed.**

Plugin nodes are plain interfaces with a `kind: NodeKind` discriminator and
no methods. Authors pattern-match via `switch (node.kind)` and TS narrows the
variant to the concrete type.

```ts
function describe(node: AstNode): string {
  switch (node.kind) {
    case "CallExpression":     return `call to ${node.callee.kind}`;
    case "ImportDeclaration":  return `import from ${node.source}`;
    // exhaustive — TS errors if a kind is missed
  }
}
```

Considered alternative: class wrappers with methods on nodes
(`node.span()`, `node.findAncestor(kind)`, …). Rejected because:

- **napi cost.** Nodes cross the worker_thread boundary as serialized data
  (see `packages/check-sdk/src/plugin-host.ts`). Classes would need to be
  reconstructed on the worker side per node per file — pure overhead vs
  passing the JSON shape directly.
- **Behaviour drift.** Methods on nodes are behaviour. Behaviour evolves;
  data shapes don't. Plain interfaces freeze cleaner.
- **Additivity.** Adding a new optional field to an interface is non-breaking;
  adding a method to a class is non-breaking only by convention. The narrower
  contract is easier to reason about for compatibility.
- **Discriminated unions are the idiomatic TS shape.** Users coming from
  typescript-eslint, ts-morph in walk mode, or hand-rolled babel visitors
  already write `switch (node.type) { ... }`. We meet them there.

Helpers (`ctx.report`, `lineView.spanFor`) live on the *context* objects, not
on nodes. That keeps nodes data-only and concentrates the API surface where
authors expect it.

### Decision 2 — node names: mirror oxc's vocabulary, freeze them as cofferdam-canonical

**Status: reaffirmed (with explicit framing).**

v0 node names match oxc's identifiers character-for-character:
`CallExpression`, `ImportDeclaration`, `MemberExpression`, etc. The names live
in `@cofferdam/check-sdk`'s namespace and are part of cofferdam's public
contract — not re-exported from any oxc binding.

This is *not* "we're tied to oxc." It's: "we picked the names that are
universal across JS AST tooling (babel, typescript-eslint, ts-morph, oxc),
they happen to coincide with oxc's, and we own them now." If we swap parsers
later, the new parser produces whatever node types it has internally and
`cofferdam-ts` (or its replacement) maps them onto the cofferdam-canonical
names that plugins already program against.

Rejected alternative: cofferdam-canonical renames (`FunctionCall`, `Import`,
`MemberAccess`, …). Rejected because:

- **Learnability tax.** Every plugin author has to learn cofferdam's
  vocabulary on top of the JS AST vocabulary they already know. Pure cost,
  no semantic value.
- **Search engine cost.** "How do I find all CallExpressions" is the
  universal Stack Overflow query. "FunctionCall" is a cofferdam-only term.
- **Independence comes from ownership, not difference.** As soon as
  `CallExpression` is declared in `@cofferdam/check-sdk`, it is ours,
  regardless of whether oxc happens to use the same string.

Constraint we *do* accept: the names cannot be relied on to match oxc's
*shape*. If oxc ever splits `CallExpression` into call vs optional-call, the
cofferdam node stays unsplit (or grows a flag); plugins are not exposed to
parser-internal restructuring.

### Decision 3 — v0 node set: 5 strict + 4 borderline + 3 trim candidates

**Status: resolved 2026-05-04 (cd-717 trim) + 2026-05-05 (cd-4de Q4
adds Program).** v0 set is **9 kinds** — 5 strict + 4 borderline.
Trim candidates already cut from the SDK on `cd-7e4-plugin-sdk-e2e`.

| Strict (named-fixture-justified) | Borderline (structural anchor / representative pattern) | Trim candidates (cut) |
|---|---|---|
| `CallExpression` | `Function` | `NewExpression` |
| `ImportDeclaration` | `ArrowFunctionExpression` | `BinaryExpression` |
| `MemberExpression` | `Class` | `StringLiteral` |
| `IdentifierReference` | `Program` (Q4 — types `view.root`) | `NumericLiteral` |
| `ObjectExpression` | | |

The 4 trim candidates have no fixture pressure and no structural-anchor
role (no other node points at them). The borderlines are kept because
`Function` / `ArrowFunctionExpression` are load-bearing for the *next*
obvious tier of plugin checks (complexity, parameter shape, async/sync
mismatch), `Class` gives plugins a typed handle for class-level checks
before deeper structure ships, and `Program` is required to type
`view.root` once the AST wire format ships (see
`design/sdk-ast-wire.md`'s Q4 — without it, `root` narrows to `never`
on any TypeScript switch).

## v0 surface — the table

Every entry below is part of cofferdam's public TS contract from v0 onward.
Removing or renaming any of them is a major-version break.

### `SourceFile`

The per-file value passed to `Check.run(file, ctx, opts)`.

| Field | Type | Justification |
|---|---|---|
| `path` | `string` | Every fixture: identifies the file in `ctx.report` output and `cofferdam.toml` glob matching. Forward-slashed on every host. |
| `text` | `string` | `brand-casing`: feeds the `LineView` iteration. `tenant-isolation`: lets a plugin slice out call-site source for the report message. |
| `lines()` | `IterableIterator<LineView>` | `brand-casing`: the entire pattern-A surface. |
| `ast` | `AstView \| null` | `no-http-client` + `tenant-isolation`: pattern-B and pattern-C entry. Nullable because parse-failed files emit `Warning.ParseError` already and skip plugin invocation; declaring the null branch keeps plugins honest. |

### `LineView`

Already locked by cd-81a.1 + cd-0ne (LineView API + classification flags).
Listed here for completeness — its public shape is part of v0.

| Field | Type | Justification |
|---|---|---|
| `lineNo` | `number` (1-based) | `brand-casing`: builds the output span; needed by every line-walk check. |
| `text` | `string` (CRLF stripped) | `brand-casing`: regex matching against trigger words. |
| `isComment` | `boolean` | `brand-casing`: skip comment-only lines. |
| `isDocComment` | `boolean` | `brand-casing`: skip JSDoc; precedent for "doc-block-aware" checks. |
| `isStringLiteral` | `boolean` | `brand-casing`: only flag user-facing string occurrences. |
| `isJsxText` | `boolean` | `brand-casing`: JSX text is user-facing too. cd-0ne. |
| `isPragma` | `boolean` | `brand-casing`: skip `/* @vite-ignore */`-style annotations. |
| `spanFor(charStart, charEnd): Span` | helper | `brand-casing`: builds a file-absolute `Span` for `ctx.report` from a within-line match. cd-cgd. |

### Strict `AstNode` kinds

#### `CallExpression`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"CallExpression"` | discriminant |
| `span` | `Span` | `no-http-client`: highlight the offending call. |
| `callee` | `AstNode` | `no-http-client`: inspect `axios.get` (a `MemberExpression`) or `axios()` (an `IdentifierReference`). `tenant-isolation`: inspect `prisma.X.findMany`. |
| `arguments` | `readonly AstNode[]` | `tenant-isolation`: the first argument is the `ObjectExpression` carrying `where` / tenant scoping. |

#### `ImportDeclaration`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"ImportDeclaration"` | discriminant |
| `span` | `Span` | `no-http-client`: highlight the banned import. |
| `source` | `string` | `no-http-client`: match `axios` / `node-fetch` / `got` / `undici`. `tenant-isolation`: detect `withTenantScope` source modules. |
| `specifiers` | `readonly { localName: string; imported?: string }[]` | `tenant-isolation`: detect that the wrapper helper was imported under a recognized name regardless of source-module rename. |

#### `MemberExpression`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"MemberExpression"` | discriminant |
| `span` | `Span` | `no-http-client`: highlight `axios.get` specifically rather than the full call. |
| `object` | `AstNode` | `no-http-client`: the `axios` left-hand side. `tenant-isolation`: walk leftward from `prisma.X.findMany` to identify the prisma client. |
| `property` | `string \| undefined` | `no-http-client`: distinguish `.get` / `.post`. `tenant-isolation`: distinguish `.findMany` / `.findFirst` / `.findUnique`. `undefined` when the property is a non-static expression (`obj[fn()]`). |
| `computed` | `boolean` | Disambiguate `.get` from `["get"]`; both are real in prod TS. |

#### `IdentifierReference`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"IdentifierReference"` | discriminant |
| `span` | `Span` | optional report target |
| `name` | `string` | `no-http-client`: identify a bare `axios()` call. `tenant-isolation`: recognize `prisma` / wrapper-helper identifiers anchoring the call chain. |

#### `ObjectExpression`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"ObjectExpression"` | discriminant |
| `span` | `Span` | `tenant-isolation`: report on the `where` clause when tenant field is missing. |
| `properties` | `readonly AstNode[]` | `tenant-isolation`: scan for the configured `tenantFields` (`siteId`, `merchantId`, …). |

> **Open**: `properties` currently exposes `AstNode[]`, but the elements are
> in practice `Property` / `SpreadElement` shapes that v0 does *not* expose
> as their own kinds. A plugin scanning `where: { siteId }` walks via the
> generic `AstNode` and falls back to `kind` checks. **Decision needed**:
> either ship `Property` and `SpreadElement` in v0 (adds two kinds) or
> document `properties` as opaque-but-iterable and require plugins to walk
> via `view.walk()` for property-key inspection. **Recommendation**: opaque
> in v0; ship `Property` only when a fixture demands it.

### Borderline `AstNode` kinds (recommended for v0)

#### `Function`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"Function"` | discriminant |
| `span` | `Span` | report target |
| `name` | `string \| undefined` | named-vs-anonymous distinction is the first thing every function-shape check needs. |
| `params` | `readonly AstNode[]` | parameter-count / parameter-shape checks (representative: cofferdam built-in `Design.MaxParameters` — proves the field has shipping demand). |
| `async` | `boolean` | async/sync mismatch checks (representative pattern; no in-tree fixture yet). |
| `generator` | `boolean` | symmetry with `async`; cost is one bool. |

#### `ArrowFunctionExpression`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"ArrowFunctionExpression"` | discriminant |
| `span` | `Span` | report target |
| `params` | `readonly AstNode[]` | every "function-shape" check has to handle both regular and arrow functions or it has dead spots. |
| `async` | `boolean` | symmetry with `Function.async`. |
| `expression` | `boolean` | distinguish `(x) => x` from `(x) => { return x; }`; needed by stylistic checks. |

#### `Class`

| Field | Type | Justification |
|---|---|---|
| `kind` | `"Class"` | discriminant |
| `span` | `Span` | report target |
| `name` | `string \| undefined` | "every class must export a name" / casing-style checks. Representative pattern. |

> **Open**: v0 omits `body` / `methods` / `extends`. Plugins needing those
> use `view.root` and walk manually. Acceptable because no named fixture
> needs them; the fact `Class` is in the union at all is to give plugins
> a typed handle for class-level checks before deeper structure ships.

#### `ObjectExpression`

(See strict block above — moved up because `tenant-isolation` justifies it.)

### Trim candidates (recommend deferring out of v0)

| Kind | Currently in SDK? | Why trim from v0 | When to add back |
|---|---|---|---|
| `NewExpression` | yes | No fixture on the board justifies it. Built-in `Warning.NoEval` uses `new Function(...)` but that's an in-engine check, not a plugin pattern. Cost of carrying it: one more variant in the discriminated union and one more visitor method. | When a plugin fixture (e.g. ban `new XMLHttpRequest()`) lands. |
| `BinaryExpression` | yes | No fixture justifies it. Built-in `Warning.TripleEquals` uses it; `Refactor.PreferNullishCoalescing` uses it — both built-in. Plugins haven't asked. | When the first plugin scoring binary operators (e.g. "no string concatenation in templates") lands. |
| `StringLiteral` | yes | `brand-casing` uses `LineView.isStringLiteral` not the AST string node; `no-http-client` uses `ImportDeclaration.source` not the literal node. No active demand. | When a plugin needs literal *content* (vs presence) outside import context. |
| `NumericLiteral` | yes | Hypothetical `NoFloatPrices` would use it; not on the board. | When the fixture lands. |

**Recommendation**: trim these four from `@cofferdam/check-sdk` v0.1.x before
publishing to npm. Internal additions are non-breaking; removals after publish
are breaking. Now is cheap; later is forever.

If trimming is rejected (e.g. "ergonomic completeness > minimalism"),
document that the four are in v0 explicitly *without* a fixture justification,
so future-us doesn't re-litigate the question.

## `Walk` and `AstVisitor`

The visitor surface used by `view.walk(visitor)` (Pattern C / `tenant-isolation`).

### `Walk`

```ts
const Walk = { Continue: "continue", Skip: "skip" } as const;
type Walk = "continue" | "skip";
```

| Variant | Justification |
|---|---|
| `Continue` | Default behaviour — descend into children. Every visitor not asking to short-circuit returns this. |
| `Skip` | `tenant-isolation` skips into wrapped-call subtrees once the wrapper helper is matched at the root, to avoid double-counting. Mirrors `oxc_ast_visit::Visit::should_walk` but lifted to the plugin surface. |

**Whole-tree termination is not exposed.** Authors needing it use `view.root`
and write their own walk; the bead's acceptance criterion ("`return false` to
stop descent") is satisfied by `Walk.Skip` per-node. Adding a third variant
later is non-breaking, removing one is breaking — start narrow.

### `AstVisitor`

One method per node kind in the v0 surface. Every method optional; the
default for an unimplemented method is `Walk.Continue`.

```ts
interface AstVisitor {
  visitCallExpression?(node: CallExpressionNode): Walk;
  visitImportDeclaration?(node: ImportDeclarationNode): Walk;
  visitMemberExpression?(node: MemberExpressionNode): Walk;
  visitIdentifierReference?(node: IdentifierReferenceNode): Walk;
  visitObjectExpression?(node: ObjectExpressionNode): Walk;
  visitFunction?(node: FunctionNode): Walk;
  visitArrowFunctionExpression?(node: ArrowFunctionExpressionNode): Walk;
  visitClass?(node: ClassNode): Walk;
}
```

(If trim candidates are deferred, drop their visitor methods too. If they
ship, add the four corresponding methods.)

| Method | Justifying fixture |
|---|---|
| `visitCallExpression` | `tenant-isolation`: accumulate prisma model calls. |
| `visitImportDeclaration` | `tenant-isolation`: detect wrapper-helper import to flip `hasTenantWrapper`. |
| `visitMemberExpression` | `tenant-isolation`: distinguish `prisma.X.findMany` from sibling member chains during walk. |
| `visitIdentifierReference` | `tenant-isolation`: recognize bare `prisma` references. |
| `visitObjectExpression` | `tenant-isolation`: examine the `where` clause inline rather than re-walking from arguments. |
| `visitFunction` / `visitArrowFunctionExpression` | borderline, see § Borderline. |
| `visitClass` | borderline, see § Borderline. |

## `AstView`

```ts
interface AstView {
  readonly root: AstNode;
  findAll<K extends NodeKind>(kind: K): readonly NodeOfKind<K>[];
  walk(visitor: AstVisitor): void;
}
```

| Member | Justification |
|---|---|
| `root` | Escape hatch when the typed surface doesn't cover a case. `tenant-isolation` does not need it; future plugins might. Cheap to keep. |
| `findAll<K>(kind)` | `no-http-client`: pattern-B entry. The generic `K extends NodeKind` is what makes the return type tight (`readonly NodeOfKind<K>[]`) — without it, plugins lose narrowing and have to cast. |
| `walk(visitor)` | `tenant-isolation`: pattern-C entry. |

## `CheckContext` — `ctx.report(args)`

The only ctx helper in v0.

```ts
interface CheckContext {
  report(args: ReportArgs): void;
}
interface ReportArgs {
  readonly message: string;
  readonly span: Span;
  readonly severity?: Severity;
  readonly related?: readonly RelatedSpan[];
  readonly fix?: Fix;
}
```

| Field | Justification |
|---|---|
| `message` | every fixture |
| `span` | every fixture; comes from `lineView.spanFor` (Pattern A) or `node.span` (B/C) |
| `severity` | `tenant-isolation`: per-finding override (a wrapped-but-shadowed call is `medium`; an unwrapped call is `high`). The default comes from `defineCheck.defaultSeverity`. |
| `related` | `tenant-isolation`: report the unscoped query and link the *file's* wrapper-import-or-lack-thereof as related context. |
| `fix` | `brand-casing` (escape hatch): replace `Rovikore` with `ROVIKORE`. Locked by cd-81a.6. |

**v0 deliberately omits**: `ctx.replaceText(span, str)`,
`ctx.insertBefore(span, str)`, `ctx.insertAfter(span, str)`,
`ctx.removeRange(span)`. The design doc that proposed those (cd-81a.6's
plan) deferred them when the `Fix { span, replacement }` shape proved
expressive enough for the e2e fixtures. They re-open if a plugin hits an
ergonomic wall (most likely "I want to insert without thinking about
adjacent spans").

## `Span` and `RelatedSpan`

Locked by cd-81a.1 + cd-81a.6. Listed here because they cross the public
boundary in `ctx.report` and `lineView.spanFor`.

```ts
interface Span {
  readonly line: number;        // 1-based
  readonly column: number;      // 1-based, UTF-8 bytes from line start
  readonly start_byte: number;  // 0-based file-absolute
  readonly end_byte: number;    // 0-based file-absolute, exclusive
}
interface RelatedSpan {
  readonly file: string;
  readonly span: Span;
}
```

Every field justified by every fixture: `cofferdam.toml` baselines hash on
file-absolute byte offsets; CLI output formatters render line/column;
autofix needs byte offsets. No trim candidates here.

## Out of scope for v0

Explicitly NOT in the v0 freeze:

- **`Property` / `SpreadElement` / `ObjectProperty` shapes.** Plugins
  iterate `ObjectExpression.properties` as `AstNode[]` and pattern-match
  via `view.walk()` if they need property-key inspection. Adding these is
  non-breaking; gates on a fixture demand.
- **JSX nodes** (`JSXElement`, `JSXAttribute`, `JSXText`, …). `brand-casing`
  uses `LineView.isJsxText` instead of the JSX AST; sufficient for v0. Real
  JSX-shape plugins (a11y rules, lint-against-component-naming) go in a
  follow-on tier.
- **TS-type nodes** (`TSTypeAnnotation`, `TSInterfaceDeclaration`, …).
  `requires_types` is an opt-in capability that routes through ts-morph in
  phase 5; the AST surface for those nodes is its own design exercise.
- **Comment nodes.** `LineView` flags cover the only comment-aware patterns
  the three fixtures need. Plugins that want raw comment text use
  `lineView.text`.
- **Scope analysis** (`Reference`, `Scope`, `Binding`). Genuinely useful for
  plugins (`no-shadow`, `no-unused-vars`) but a separate architectural axis
  — postpone until at least one fixture demands it.

## CI guardrails for the freeze

To make the v0 surface mechanically enforceable after this doc lands:

1. **Public API typecheck.** A test in `packages/check-sdk/tests/positive.ts`
   imports each of the v0 names and uses each field. Removing or renaming
   any of them fails the test.
2. **Discriminated-union exhaustiveness.** A `switch (node.kind)` test that
   covers every kind, with a `case _: const _exhaustive: never = node;`
   guard. Adding a kind without updating the test fails compile.
3. **No hidden surface.** A grep step that fails if `packages/check-sdk/src/`
   exports anything not named in this doc's table. Implementation:
   `grep -E '^export ' packages/check-sdk/src/index.ts | sort` against a
   committed snapshot.
4. **No `oxc_*` strings in `dist/`.** Mirrors the platform-extensibility
   guardrail at the SDK level — if any of our serialized JS leaks an oxc
   identifier, we've broken the abstraction.

The cd-81a.2 acceptance bullet is "three rovikore checks implementable with
no SDK gaps" — guardrails 1–3 raise that to "implementable AND the surface
is tight."

## Decisions still open — resolve before turning this into the cd-81a.2 close

1. ~~**Trim or keep the four trim candidates?**~~ **Resolved 2026-05-04: trim.**
   `NewExpression`, `BinaryExpression`, `StringLiteral`, `NumericLiteral`
   removed from the v0 `NodeKind` union, the `AstNode` discriminated union,
   the `AstVisitor` interface, and the `@cofferdam/check-sdk` index
   re-exports on `cd-7e4-plugin-sdk-e2e`. Verified: `tsc --noEmit` clean,
   `tests/positive.ts` + `tests/negative.ts` typecheck, `plugin-host.test.mjs`
   6/6 pass, `scripts/platform-purity.mjs` 4/4 pass. Built-in Rust checks
   (`cofferdam-checks`) keep their oxc node access — the trim is purely on
   the public TS contract.
2. **Ship `Property` in v0 alongside `ObjectExpression`?** Recommendation:
   no, defer. `tenant-isolation` works without it via `view.walk()`.
3. **Should `ctx.report` accept a `lineNo` shortcut for line-walk plugins?**
   Currently authors call `lineView.spanFor(start, end)`. Adding
   `ctx.reportLine({ message, lineView, charStart, charEnd })` is sugar that
   would shorten `brand-casing` by ~3 lines per finding. Recommendation:
   defer; `spanFor` is one line and the API stays smaller.
4. **Visitor signature: methods on an interface vs a single dispatcher?**
   Currently methods. An alternative is `view.walk((node) => Walk)` with a
   single switch. Methods cost one prototype property per kind; dispatcher
   costs one switch in user code. Recommendation: keep methods — they pair
   cleaner with `Walk.Skip` per-kind decisions, which is the whole point
   of the visitor over `findAll`.
5. **Should this doc become an ADR?** Probably yes once the platform-split
   ADR lands (`docs/adr/0001-platform-split.md`), since both freeze public
   contracts. Recommendation: leave as `design/sdk-ast-surface.md` until the
   ADR habit is established repo-wide, then move both together.

## What "done" looks like

Acceptance for this doc:

- The table in [§ v0 surface — the table](#v0-surface--the-table) accounts
  for every export from `packages/check-sdk/src/index.ts`.
- Every entry has a one-line fixture justification or an explicit
  "borderline / trim" tag.
- Decisions 1, 2, 3 above are resolved (in-doc) before cd-81a.2 closes.
- A follow-on bead implements guardrails 1–4.

Acceptance for cd-81a.2 itself (which this doc gates):

- Three named fixtures (BrandCasing, NoHttpClient, TenantIsolation) ship
  green using only the v0 surface frozen here.
- Zero `oxc_*` identifiers anywhere in `packages/check-sdk/src/` or
  `examples-plugins/*/src/`.
- The trim candidates are either out of v0 or have an explicit "kept
  without fixture" line in this doc.
