# Plugin SDK end-to-end fixture (cd-7e4)

Status: design-only. The SDK epic (cd-81a) has no completed children, so this
document describes the *target shape* the SDK must satisfy when it lands. It
also doubles as the acceptance contract for cd-7e4.

## 1. Why BrandCasing

Source: `C:/Users/tajdi/rovikore-host/backend/dev_checks/rovikore_host_credo/brand_casing.ex`.
That Elixir/Credo check flags occurrences of `Rovikore` (sentence case) in
user-facing surfaces, exempting:

- module identifiers (`alias Mix.Tasks.Rovikore.Gen.ApiKey`, `defmodule …`)
- comments (`# foo Rovikore bar`)
- doc lines (`@moduledoc`, `@doc`, `@shortdoc`, `@typedoc`)
- the `dev_checks/` directory itself (the check references the trigger word)
- any line preceded by `# brand:ignore — <why>`

It is the smallest of the three rovikore-host acceptance targets named in
cd-81a.2 and exercises **only** Pattern A from the SDK design (line walk +
magic-comment exemption). It does not need the AST surface (cd-81a.2) to be
feature-complete to run, only to be wired enough that `file.lines()` can
classify lines using token data — which means it doubles as a smoke test that
LineView + the loader work before AST findAll/walk are stable.

NoHttpClient (Pattern B) and TenantIsolation (Pattern C) get sibling fixtures
later (see §6).

The TS port flags `Rovikore` in:

- string literals (single, double, template — these are user-facing copy)
- JSX text and attribute values (real display copy in React/HTML output)

and exempts:

- comments (`// …`, `/* … */`)
- JSDoc / doc comments (`/** … */`)
- identifiers (imports, type names, class/function names — `import { Rovikore }`,
  `class RovikoreClient`, `Rovikore.foo()`)
- any line preceded by `// brand:ignore — <why>` (plugin-level escape hatch)
- any line covered by `// cofferdam-ignore: BrandCasing` (engine-level
  suppression from cd-81a.4 — exercised explicitly per acceptance criteria)

## 2. Directory layout

```
examples-plugins/
  brand-casing/
    package.json          # depends on @cofferdam/check-sdk
    tsconfig.json         # strict; emits to dist/ for the loader
    src/
      index.ts            # defineCheck(...) — the check authoring surface
    fixture.ts            # the .ts file fed to `cofferdam check`
    expected.json         # golden JSON output (committed)
    README.md             # short — "what this fixture proves"
```

Cofferdam config that wires it in (at the *repo root*, not inside the plugin):

```toml
# cofferdam.toml
plugins = ["./examples-plugins/brand-casing"]

[checks."BrandCasing"]
# Default options come from the plugin's defineCheck schema; this block exists
# to prove the cofferdam.toml override path works (cd-81a.3) AND to document
# the surface for the README.
brand = "ROVIKORE"
allowedAliases = ["RovikoreClient", "RovikoreCdn"]
```

### `package.json`

```json
{
  "name": "@cofferdam-fixtures/brand-casing",
  "private": true,
  "version": "0.0.0",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc -p .",
    "build:fail": "tsc -p ./tsconfig.fail.json"
  },
  "dependencies": {
    "@cofferdam/check-sdk": "workspace:*"
  },
  "devDependencies": {
    "typescript": "^5.6.0"
  }
}
```

`build:fail` builds the *negative* fixture (`src/index.fail.ts`, see §3) and
must exit non-zero. CI asserts on that.

### `tsconfig.json`

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "outDir": "dist",
    "declaration": true
  },
  "include": ["src/index.ts"]
}
```

`tsconfig.fail.json` extends this and includes `src/index.fail.ts` instead.

## 3. The check — TS pseudocode

`src/index.ts` (this is the canonical "≤30 lines" target from cd-81a.8's
acceptance criteria — keep the body tight):

```ts
import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

const TRIGGER = /\bRovikore\b/;
const PLUGIN_IGNORE = /\bbrand:ignore\b/;

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  explanation:
    "Brand name must be all-caps ROVIKORE in user-facing copy. " +
    'Add `// brand:ignore — <why>` on the previous line for legitimate references.',
  options: {
    brand: { default: "ROVIKORE", type: "string" },
    allowedAliases: { default: [], type: "string[]" },
  },
  run(file, ctx, opts) {
    const lines = [...file.lines()];
    const ignoredNext = new Set<number>();
    for (const ln of lines) {
      if (PLUGIN_IGNORE.test(ln.text)) ignoredNext.add(ln.lineNo + 1);
    }

    for (const ln of lines) {
      if (ignoredNext.has(ln.lineNo)) continue;
      if (ln.isComment || ln.isDocComment || ln.isPragma) continue;
      // Identifier-only lines: skip when no string/JSX context on the line.
      if (!ln.isStringLiteral && !ln.isJsxText) continue;

      const m = TRIGGER.exec(ln.text);
      if (!m) continue;
      if (opts.allowedAliases.some((a) => ln.text.includes(a))) continue;

      ctx.report({
        message: `Brand name must be "${opts.brand}", not "${m[0]}".`,
        severity: Severity.Warning,
        span: ln.spanFor(m.index, m.index + m[0].length), // byte offsets, not chars
      });
    }
  },
});
```

### API surface this exercises

| Surface                       | Source bead | Used as                                   |
| ----------------------------- | ----------- | ----------------------------------------- |
| `defineCheck(...)`            | cd-81a.8    | The factory itself; typed return.         |
| `Category` / `Severity` enums | cd-81a.8    | Identical values to the Rust enums.       |
| `file.lines()` iterator       | cd-81a.1    | LineView — drives the whole walk.         |
| `LineView.isComment`          | cd-81a.1    | Skip `// foo Rovikore`.                   |
| `LineView.isDocComment`       | cd-81a.1    | Skip `/** Rovikore */`.                   |
| `LineView.isStringLiteral`    | cd-81a.1    | Only flag inside `"…"` / `'…'` / `` `…` `` |
| `LineView.isJsxText` (new)    | cd-81a.1?   | Flag JSX text content. See §6.            |
| `LineView.isPragma`           | cd-81a.1    | Skip `// @ts-…`, `/** @jsx … */`.         |
| `LineView.spanFor(s,e)`       | cd-81a.1    | Byte-offset span for `ctx.report`.        |
| `ctx.report({...})`           | cd-81a.6    | Issue emission — span must round-trip.    |
| `opts` typing inference       | cd-81a.3    | `opts.brand: string`, `opts.allowedAliases: string[]` |
| Plugin loader handshake       | cd-81a.7    | Loaded via `plugins = [...]` in toml.     |

`LineView.isJsxText` is *not* in cd-81a.1's acceptance list today. Either we
add it (preferred — JSX is core to TS), or the fixture restricts itself to
string literals only and the JSX case becomes a sibling fixture. See §6 for
the proposed bead.

### Negative fixture — `src/index.fail.ts`

Same body, but with one deliberately wrong AST property access to prove
cd-81a.2's type tightness from cd-81a.8's README claim ("invalid AST property
access fails to compile"):

```ts
// EXPECTED to fail tsc; CI asserts `pnpm build:fail` exits non-zero.
import { defineCheck } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "BrandCasing",
  category: 999 as any,            // not the failing line — cast hides it
  basePriority: 15,
  explanation: "...",
  options: {},
  run(file, ctx) {
    // @ts-expect-error — file.lyne is not a method; replace with file.lines()
    for (const ln of file.lyne()) {
      ctx.report({ message: "x", severity: 1, span: ln.span });
    }
  },
});
```

CI assertion: `tsc -p tsconfig.fail.json` must exit **zero**. With
`@ts-expect-error`, the directive expects the next line to fail; if the
line stops failing (the SDK loosened its types), tsc reports the
directive itself as unused (TS2578) and the build fails.

This is tighter than the "raw tsc must fail" approach — it pins the
exact lines where the type system is supposed to fire, not just the
overall pass/fail state.

## 4. The fixture — `fixture.ts`

```ts
// fixture.ts — input to `cofferdam check`. Comments label expected outcomes.

import { Rovikore } from "./brand";        // OK: identifier import
import type { RovikoreClient } from "./b"; // OK: type identifier

class RovikoreSdk {                         // OK: identifier (class name)
  greet(): string {
    return "Welcome to Rovikore!";          // FLAG #1: string literal
  }
}

export const HEADER = `Rovikore — go faster`; // FLAG #2: template literal

// Below the brand:ignore line — plugin-level escape hatch from rovikore.
// brand:ignore — legacy fixture asserting on the old casing
export const LEGACY = "Rovikore (legacy)";  // EXEMPT (plugin magic comment)

// Engine-level suppression from cd-81a.4 — different mechanism, same effect.
// cofferdam-ignore: BrandCasing: see ROVI-481 — copywriter approved exception
export const CAMPAIGN = "Rovikore Spring Sale"; // EXEMPT (engine suppression)

// Comment with the trigger word: Rovikore is fine in dev context. // EXEMPT
/** JSDoc mentioning Rovikore in passing. */                       // EXEMPT
/* Block comment: Rovikore here is also fine. */                   // EXEMPT

export function ok(): string {
  return "ROVIKORE all caps — fine.";       // OK: brand spelled correctly
}
```

Expected: 2 issues (lines marked `FLAG #1` and `FLAG #2`), nothing else.

## 5. `expected.json` and the CI shape

### Golden file format

`expected.json` is the JSON output of `cofferdam check fixture.ts --format
json --pretty`, with the path field normalised to repo-relative POSIX form.

Schema is the existing JSON formatter contract — see
`cofferdam-formatters/src/json.rs::RobotReport`. The committed file looks
like:

```json
{
  "findings": [
    {
      "id": "BrandCasing",
      "category": "warning",
      "priority": 15,
      "severity": "warning",
      "file": "examples-plugins/brand-casing/fixture.ts",
      "line": 11,
      "column": 13,
      "start_byte": 0,
      "end_byte": 0,
      "message": "Brand name must be \"ROVIKORE\", not \"Rovikore\"."
    },
    {
      "id": "BrandCasing",
      "category": "warning",
      "priority": 15,
      "severity": "warning",
      "file": "examples-plugins/brand-casing/fixture.ts",
      "line": 14,
      "column": 25,
      "start_byte": 0,
      "end_byte": 0,
      "message": "Brand name must be \"ROVIKORE\", not \"Rovikore\"."
    }
  ],
  "summary": {
    "total": 2,
    "by_category": { "warning": 2 }
  }
}
```

`start_byte` / `end_byte` are zero-filled until the loader runs and the real
offsets are recorded. Line/column numbers come from `fixture.ts` directly:
the FLAG #1 string-literal `Rovikore` is on line 11 of the fixture
(`return "Welcome to Rovikore!";`), and FLAG #2 (template literal) is on
line 14 (`export const HEADER = ` + backticks). Regenerate everything by
running `cofferdam check examples-plugins/brand-casing/fixture.ts --format
json --pretty | jq .` once the SDK lands.

### Round-trip check

A separate script asserts that for each issue, slicing the source by
`byte_start..byte_end` returns the literal string `"Rovikore"`. This is the
tangible cd-81a.2 acceptance bullet — *spans round-trip back to the original
source* — not just a JSON-shape diff.

### Diffing

CI step (in a new `.github/workflows/plugin-sdk-e2e.yml` matrix or appended
to the existing `cofferdam-check.yml`):

```yaml
- name: Build plugin
  run: pnpm --filter brand-casing build

- name: Run cofferdam against fixture
  run: |
    cargo run --release -p cofferdam-cli -- check \
      examples-plugins/brand-casing/fixture.ts \
      --format json --pretty > actual.json

- name: Diff against golden
  # The formatter already forward-slashes paths and emits stable field
  # ordering (serde + BTreeMap), so a plain diff is enough. If host quirks
  # show up later, normalise both sides through `jq -S .` first.
  run: diff -u examples-plugins/brand-casing/expected.json actual.json

- name: Round-trip span check
  run: node scripts/check-spans.mjs actual.json examples-plugins/brand-casing/fixture.ts Rovikore

- name: Negative fixture pins SDK type strictness
  # tsc -p tsconfig.fail.json must exit zero — every `@ts-expect-error`
  # in src/index.fail.ts must match a real type error. If the SDK ever
  # loosens, one directive becomes unused and tsc fails the build.
  run: pnpm --filter brand-casing build:fail
```

### Regeneration

Three regen surfaces, in increasing trust requirement:

1. **Local dev**: `cargo make regen-plugin-fixtures` (or `just`) — regenerates
   `expected.json` from the live cofferdam binary. Diff is committed by hand.
2. **CI insight comment**: on diff failure, CI uploads `actual.norm.json` as
   a job artifact + posts a PR comment with the unified diff. Author copies
   the new file in if the change is intentional.
3. **`bd` flow**: changes to `expected.json` larger than ±1 issue require a
   linked bead (`cd-…-fixture-update: BrandCasing golden changed because …`).
   This is convention, not enforced by CI.

## 6. Sibling fixtures (out of scope for cd-7e4, but design-relevant)

The point of the BrandCasing fixture is to land Pattern A (line walk +
magic-comment exemption) end-to-end. Pattern B and Pattern C from cd-81a's
description need their own fixtures once the SDK lands.

A note on porting fidelity: cd-81a's description names rovikore checks like
TenantIsolation and ApiFirst as *drivers* for the API design — they exist in
Elixir/Phoenix-with-Ash and don't have literal TypeScript analogues. The
sibling fixtures should exercise the same SDK *pattern* against a target
that fires in real TS code, not a contrived port. Concrete picks:

| Fixture                 | Pattern  | API surface exercised                               | What it flags                                                              |
| ----------------------- | -------- | --------------------------------------------------- | -------------------------------------------------------------------------- |
| `no-http-client`        | B (AST findAll) | `file.ast.findAll(ImportDeclaration)`, `findAll(CallExpression)` | `import` of `axios` / `node-fetch` / `got` outside an allowlisted wrapper. Direct port of NoBannedHttpClient — the pattern is framework-agnostic. |
| `tenant-isolation`      | C (stateful walk) | `file.ast.walk(visitor)` with accumulator + per-file decision | Prisma model queries (`prisma.user.findMany(...)`) where the `where` clause omits a tenant field declared in options. The bug class is tenant isolation regardless of ORM; the fixture happens to use Prisma but the check ID is framework-agnostic so a Drizzle/Kysely sibling slots in later without rename churn. |
| `brand-casing-jsx`      | A extension | LineView + `isJsxText`                          | Splits the JSX-text half of BrandCasing into its own fixture if cd-81a.1 ships without `isJsxText` (see §7 question 1). |

`tenant-isolation` accumulator shape (matches the original TenantIsolation
pattern, mapped to TS via Prisma):

```ts
const state = {
  imports: new Set<string>(),       // names imported from "@prisma/client" or "./prisma"
  unscopedQueries: [] as Span[],    // findMany/findFirst/findUnique calls missing tenant field
  hasTenantWrapper: false,          // file imports a `withTenantScope`/`scopedPrisma` helper
};
```

Final-pass: emit one issue per accumulated `unscopedQueries` span if
`!state.hasTenantWrapper`. This is the same shape as
`%{ash_resource, found_fields, has_multitenancy, module_parts}` in
TenantIsolation — same SDK demands, real TS signal.

The CI shape from §5 (build → run → diff → round-trip → fail-fixture) applies
unchanged. Each fixture is a separate `examples-plugins/<name>/` directory
with the same five files; the toml `plugins = [...]` array grows.

Out of scope for the sibling work: NoFloatPrices (Phoenix-shaped, niche),
ApiFirst (Phoenix contracts, no clean TS analogue). If a Pattern B/C signal
turns up later that's worth a fixture, file a fresh bead — don't force a
literal port from rovikore.

## 7. ts-morph routing status (0.2.x)

`DefineCheckInput.requiresTypes` (and the matching `Check.requiresTypes` field) is accepted by the type system and propagated through `defineCheck`, but **type-aware routing via ts-morph is not wired in the 0.2.x line**. A plugin that sets `requiresTypes: true` will still run — its `run()` callback fires per file exactly as it would for any other check — but no type information is injected and the engine does not invoke ts-morph before or after the call. To avoid silent surprises, the plugin host emits a one-time warning to stderr at load time when it encounters a plugin with `requiresTypes: true`: `[cofferdam] plugin "<id>" sets requiresTypes:true — type-aware routing (ts-morph) is not yet wired in 0.2.x; the check will run without type information. Track cd-l58 / gh #16 for status.` Plugin authors should treat the field as a declaration of intent and not depend on type data being present until the wiring lands. Track cd-l58 / [gh #16](https://github.com/TAJD/cofferdam/issues/16) for status.

## 8. Open questions for the SDK epic

These are decisions the SDK epic owners need to make; logging here so they
don't get lost when cd-7e4 starts.

1. **JSX classification.** Is `LineView.isJsxText` in scope for cd-81a.1, or
   does BrandCasing only assert on string literals (and the JSX case becomes
   a sibling fixture)? The fixture above uses both; restricting it loses
   coverage of an obvious user-facing surface.

2. **Magic-comment scoping.** `// brand:ignore` is a *plugin-defined* marker;
   `// cofferdam-ignore: BrandCasing` is the *engine-defined* one (cd-81a.4).
   Both must coexist. The fixture exercises both deliberately. SDK docs need
   to spell out the precedence: engine suppression filters the final issue
   list, plugin magic comments filter inside `run()`.

3. **`spanFor` ergonomics.** The pseudocode uses
   `ln.spanFor(m.index, m.index+m[0].length)`. cd-81a.1 needs to commit to a
   helper this concise — if it's `new Span(file, line, col, ...)` instead, the
   "≤30 lines" target from cd-81a.8 slips.

4. **Options-defaults visibility.** When `cofferdam.toml` overrides only one
   field, the others must come from `defineCheck.options.*.default` — this is
   the specific path cd-81a.3's "defaults work without any config" bullet
   maps to. The fixture leaves `allowedAliases` unset to test it.

5. **`workspace:*` resolution at runtime.** cd-81a.7's loader needs to
   resolve `@cofferdam/check-sdk` from the plugin's own `node_modules`, not
   from cofferdam's bundled copy — otherwise plugin authors with a different
   SDK minor version see surprising failures. Worth pinning in the loader's
   bead.
