# Plugin SDK author guide

This guide walks you from "I want to enforce this rule in my repo" to a
published, tested plugin. It is the narrative on-ramp; for the acceptance
contract and golden-file CI shape see
[`docs/plugin-sdk-e2e.md`](./plugin-sdk-e2e.md). For the full API surface see
the [`@cofferdam/check-sdk` README on npm](https://www.npmjs.com/package/@cofferdam/check-sdk).

## 1. What a cofferdam plugin is

A plugin is a TypeScript module that `default export`s a `Check` object
produced by `defineCheck`. The cofferdam engine loads it via the `plugins =
[...]` array in `cofferdam.toml` and runs it against every file alongside the
built-in checks. Findings flow through the same priority computation,
suppression directives, baseline diffing, and output formats.

Three check shapes cover almost every rule class:

| Pattern | When to use | Primary API |
|---|---|---|
| **A — line walk** | Content in string literals or comments: brand spelling, banned words, licence headers. No AST needed. | `file.lines()` → `LineView` flags |
| **B — AST findAll** | Tag-and-match against a specific node kind: banned imports, forbidden API calls. | `file.ast.findAll("ImportDeclaration")` etc. |
| **C — stateful walk** | Rules that depend on inter-statement context: tenant scoping, accumulator/mutator pairs. | `file.ast.walk(visitor)` with accumulator |

Pattern A is the fastest and has the widest applicability. Reach for B or C
only when the rule cannot be expressed on individual lines.

The `AstView` surface exposes nineteen node kinds today: `Program`,
`CallExpression`, `ImportDeclaration`, `Function`, `ArrowFunctionExpression`,
`Class`, `ObjectExpression`, `MemberExpression`, `IdentifierReference`,
`VariableDeclaration` (CD-78 — one node per `const`/`let`/`var` statement,
exposing declarator name(s) + init), `JSXElement`, `JSXAttribute`,
`JSXExpressionContainer` (against `.ts`/`.tsx` files), plus `Document`,
`Element`, `Attribute`, `Text`, `Comment`, `Doctype` (against `.html` files,
CD-84 — see §2 below). New kinds are additive — minor releases may add them
without breaking existing plugins.

## 2. Pattern examples

### Pattern A — line walk with magic-comment exemption

**Use case:** flag the wrong casing of a brand name inside string literals and
JSX text, with a per-line escape hatch.

This is the canonical BrandCasing example from `docs/plugin-sdk-e2e.md`.
Keep the body under 30 lines; the SDK is designed to make that possible.

```ts
// src/index.ts
import { defineCheck, Category } from "@cofferdam/check-sdk";

const TRIGGER = /\bExamplco\b/;
const PLUGIN_IGNORE = /\bbrand:ignore\b/;

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  explanation:
    "Brand name must be all-caps EXAMPLCO in user-facing copy. " +
    'Add `// brand:ignore — <why>` on the previous line for legitimate references.',
  options: {
    brand: { default: "EXAMPLCO", type: "string" },
    allowedAliases: { default: [] as string[], type: "string[]" },
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
      if (!ln.isStringLiteral && !ln.isJsxText) continue;
      const m = TRIGGER.exec(ln.text);
      if (!m) continue;
      if (opts.allowedAliases.some((a) => ln.text.includes(a))) continue;
      ctx.report({
        message: `Brand name must be "${opts.brand}", not "${m[0]}".`,
        span: ln.spanFor(m.index, m.index + m[0].length),
      });
    }
  },
});
```

Key points:

- `file.lines()` returns an `IterableIterator<LineView>`. Each `LineView`
  carries classification flags (`isComment`, `isDocComment`, `isStringLiteral`,
  `isJsxText`, `isPragma`) populated by the engine from the parsed comment list
  and an AST walk over string/template literals.
- `ln.spanFor(start, end)` takes byte offsets **relative to the start of
  `ln.text`** and returns a file-absolute `Span` for `ctx.report`. Do not
  construct spans by hand — the helper handles CRLF stripping and line-start
  accounting correctly.
- Magic-comment filtering (`brand:ignore`) runs **inside** `run()` — you
  decide which lines to skip. Engine-level suppression (`// cofferdam-ignore:
  BrandCasing`) filters after `run()` returns. Both mechanisms coexist; see
  §4 of `plugin-sdk-e2e.md` for the precise precedence.
- `opts` is typed from the `options` schema — `opts.brand` is `string`,
  `opts.allowedAliases` is `readonly string[]`. TypeScript catches mismatches
  at compile time.

### Pattern B — AST findAll

**Use case:** flag imports of banned HTTP client packages (`axios`,
`node-fetch`, `got`) unless they come from an allowlisted wrapper module.

```ts
// src/index.ts
import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

const BANNED = new Set(["axios", "node-fetch", "got", "superagent", "request"]);

export default defineCheck({
  id: "NoHttpClient",
  category: Category.Warning,
  basePriority: 15,
  explanation:
    "Direct HTTP client imports are banned. " +
    "Import from the project wrapper (e.g. `lib/http.ts`) instead.",
  options: {
    allowedWrappers: { default: [] as string[], type: "string[]" },
  },
  files: { extensions: ["ts", "tsx"] },
  run(file, ctx, opts) {
    if (!file.ast) return;
    for (const node of file.ast.findAll("ImportDeclaration")) {
      const src = node.source;
      if (!BANNED.has(src)) continue;
      if (opts.allowedWrappers.some((w) => file.path.includes(w))) continue;
      ctx.report({
        message: `Import "${src}" is banned. Use the project HTTP wrapper.`,
        severity: Severity.High,
        span: node.span,
      });
    }
  },
});
```

Key points:

- Guard with `if (!file.ast) return;`. `file.ast` is `null` only when parsing
  failed — the engine already emitted `Warning.ParseError` for those files.
  Parsing succeeds for well-formed TypeScript, so in practice `ast` is non-null
  for every file the check is invoked on.
- `file.ast.findAll("ImportDeclaration")` returns
  `readonly ImportDeclarationNode[]`. Each node exposes `source: string` (the
  module specifier) and `specifiers` (imported names). Use `node.span` for the
  issue location.
- `files: { extensions: ["ts", "tsx"] }` is a `FileScope` filter. The engine
  applies it before calling `run()` — no per-file extension guard needed inside
  the check. The same shape also accepts `layers: ["ui", "app"]` to scope a
  check to specific layers from `cofferdam.invariants.toml` without writing
  `if (file.layer !== "ui") return;` inside `run()`.
- For `CallExpression` patterns (e.g. detecting direct `fetch(...)` calls),
  replace `findAll("ImportDeclaration")` with `findAll("CallExpression")` and
  inspect `node.callee`.

#### JSX findAll — flag `<img>` with no `alt`

**Use case:** an SEO/accessibility check that flags `<img>` elements missing an
`alt` attribute, without a `{...spread}` that might supply one dynamically.
See [SEO-grade checking](/seo-checking) for this pattern composed with three
others (cross-file HTML duplicates, type-aware literal resolution, and
`verify --dist`) into one worked plugin.

```ts
// src/index.ts
import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "ImgMissingAlt",
  category: Category.Warning,
  basePriority: 10,
  defaultSeverity: Severity.Medium,
  explanation: "An <img> element has no `alt` attribute.",
  files: { extensions: ["tsx", "jsx"] },
  run(file, ctx) {
    if (!file.ast) return;
    for (const el of file.ast.findAll("JSXElement")) {
      if (el.tagName !== "img") continue;
      const hasAlt = el.attributes.some((a) => a.isSpread || a.name === "alt");
      if (!hasAlt) {
        ctx.report({ message: "<img> is missing an `alt` attribute", span: el.span });
      }
    }
  },
});
```

Key points:

- `findAll("JSXElement")` returns `readonly JSXElementNode[]`. `tagName`
  flattens member-expression tags too — `<Image.Src />` reports
  `tagName === "Image.Src"`.
- `el.attributes` is `readonly JSXAttributeNode[]`, one entry per attribute
  item on the opening tag — including `{...spread}` attributes. Check
  `a.isSpread` before treating `a.name === undefined` as "no name given";
  a spread might supply the attribute you're looking for at runtime, so
  most checks should treat a spread as "can't tell, don't flag."
- `a.value` is the literal string for `name="value"`, and `undefined` with
  `a.isExpression === true` for `name={expr}` — the check above only cares
  whether `alt` is present at all, but a stricter check could also flag
  `alt={undefined}` via `findAll("JSXExpressionContainer")`.

#### HTML findAll — flag `<img>` with no `alt` (CD-84)

**Use case:** the same SEO/accessibility rule as above, but against `.html`
files directly (no JSX/framework involved) — a static HTML page's `<img>`
elements.

```ts
// src/index.ts
import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "HtmlImgMissingAlt",
  category: Category.Warning,
  basePriority: 10,
  defaultSeverity: Severity.Medium,
  explanation: "An <img> element has no `alt` attribute.",
  files: { extensions: ["html"] },
  run(file, ctx) {
    if (!file.ast) return;
    for (const el of file.ast.findAll("Element")) {
      if (el.tagName !== "img") continue;
      const hasAlt = el.attributes.some((a) => a.name === "alt");
      if (!hasAlt) {
        ctx.report({ message: "<img> is missing an `alt` attribute", span: el.span });
      }
    }
  },
});
```

Key points:

- `file.ast` against an `.html` file is built from tree-sitter-html
  (`crates/cofferdam-html`) rather than oxc — the same `AstView` shape,
  a different parser underneath.
- `findAll("Element")` returns `readonly ElementNode[]`: `tagName`,
  `selfClosing`, `attributes` (`readonly AttributeNode[]`), and `children`
  (non-attribute child nodes — `Text`, nested `Element`s, `Comment`).
  `AttributeNode.value` is `undefined` for a boolean attribute
  (`disabled`) and `""` (not `undefined`) for an empty quoted value
  (`alt=""`) — check which one you mean before treating a falsy value as
  "missing".
- To read an element's text content (e.g. a `<title>`'s contents), filter
  `el.children` for `c.kind === "Text"` and join `.text`:
  ```ts
  const text = el.children.filter((c) => c.kind === "Text").map((c) => c.text).join("");
  ```
- `file.lines()` against an `.html` file carries HTML-shaped classification
  flags — `isTag`, `isText`, `isComment` — instead of the JS-specific
  `isStringLiteral`/`isJsxText` (which are always `false` for `.html`
  files; `isDocComment`/`isPragma` are JS-only too and also always
  `false`).
- A duplicate-`<title>`-across-files check follows the exact same
  `ctx.corpus` + `finalize` pattern as `DuplicateClassName` above — collect
  `{ file, text, span }` records per file in `run`, group by title text in
  `finalize`, and report cross-file duplicates. See
  `packages/check-sdk/tests/html-ast-view.test.mjs` for a full working
  example (`"duplicate <title> text across two files..."`).

### Pattern C — stateful walk

**Use case:** collect all Prisma model queries in a file; flag any that omit a
`tenantId` field in the `where` clause, unless the file imports a
`withTenantScope` wrapper.

```ts
// src/index.ts
import { defineCheck, Category, Severity, Walk } from "@cofferdam/check-sdk";
import type { AstVisitor, CallExpressionNode, ImportDeclarationNode, Span } from "@cofferdam/check-sdk";

const PRISMA_QUERY_METHODS = new Set(["findMany", "findFirst", "findUnique", "findUniqueOrThrow"]);
const TENANT_WRAPPERS = ["withTenantScope", "scopedPrisma"];

export default defineCheck({
  id: "TenantIsolation",
  category: Category.Warning,
  basePriority: 20,
  explanation:
    "Prisma queries must include a tenant field or use a tenant-scoping wrapper. " +
    "Bare queries may leak cross-tenant data.",
  options: {
    tenantField: { default: "tenantId", type: "string" },
  },
  files: { extensions: ["ts", "tsx"] },
  run(file, ctx, opts) {
    if (!file.ast) return;

    const state = {
      unscopedQueries: [] as Span[],
      hasTenantWrapper: false,
    };

    const visitor: AstVisitor = {
      visitImportDeclaration(node: ImportDeclarationNode): Walk {
        if (node.specifiers.some((s) => TENANT_WRAPPERS.includes(s.localName))) {
          state.hasTenantWrapper = true;
        }
        return Walk.Continue;
      },
      visitCallExpression(node: CallExpressionNode): Walk {
        // Match prisma.<model>.<method>(...)
        if (node.callee.kind !== "MemberExpression") return Walk.Continue;
        const method = node.callee;
        if (method.kind !== "MemberExpression") return Walk.Continue;
        if (!PRISMA_QUERY_METHODS.has(method.property ?? "")) return Walk.Continue;

        // Check if the first argument object contains the tenant field.
        const args = node.arguments;
        const firstArg = args[0];
        if (!firstArg || firstArg.kind !== "ObjectExpression") {
          state.unscopedQueries.push(node.span);
          return Walk.Continue;
        }
        // ObjectExpression.properties are AstNode[]; a MemberExpression
        // property match is a heuristic — full analysis requires types.
        const hasTenantField = firstArg.properties.some(
          (p) => p.kind === "MemberExpression" && p.property === opts.tenantField,
        );
        if (!hasTenantField) {
          state.unscopedQueries.push(node.span);
        }
        return Walk.Continue;
      },
    };

    file.ast.walk(visitor);

    if (!state.hasTenantWrapper) {
      for (const span of state.unscopedQueries) {
        ctx.report({
          message: `Prisma query missing \`${opts.tenantField}\` in where clause.`,
          severity: Severity.High,
          span,
        });
      }
    }
  },
});
```

Key points:

- `file.ast.walk(visitor)` calls your visitor callbacks in document order.
  Return `Walk.Continue` to descend into children; return `Walk.Skip` to prune
  the subtree for that node only — sibling nodes still visit.
- Accumulate into local state before deciding whether to emit. The "has a
  tenant wrapper" check requires seeing the whole file before reporting.
- Pattern C often has a heuristic quality — `ObjectExpression.properties` are
  `AstNode[]` and a thorough `where`-clause analysis requires resolved types.
  Note the limitation in the `explanation` if the check is approximate.
  Full type-aware routing (ts-morph, via `requiresTypes: true`) is
  available; see §7 below.
- Override only the visitor methods you need. Omitted kinds default to
  `Walk.Continue`.

## 3. Mapping ESLint rule patterns to cofferdam plugins

ESLint rules are functions that operate on AST nodes via a listener registry.
Cofferdam checks are objects with a `run(file, ctx, opts)` callback. The
translation is straightforward.

### `no-console`

ESLint suppresses `console.log` / `console.error` etc. in production code.

```ts
// ESLint
module.exports = { create(context) { return { CallExpression(node) {
  if (node.callee.type === "MemberExpression" &&
      node.callee.object.name === "console") {
    context.report({ node, message: "Unexpected console statement." });
  }
} }; } };

// cofferdam
export default defineCheck({
  id: "NoConsole",
  category: Category.Warning,
  basePriority: 10,
  explanation: "Remove console statements before committing.",
  options: {
    allowedMethods: { default: ["warn", "error"] as string[], type: "string[]" },
  },
  run(file, ctx, opts) {
    if (!file.ast) return;
    for (const node of file.ast.findAll("CallExpression")) {
      if (node.callee.kind !== "MemberExpression") continue;
      const callee = node.callee;
      if (callee.kind !== "MemberExpression") continue;
      if (callee.object.kind !== "IdentifierReference") continue;
      if (callee.object.name !== "console") continue;
      if (opts.allowedMethods.includes(callee.property ?? "")) continue;
      ctx.report({ message: "Unexpected console statement.", span: node.span });
    }
  },
});
```

`defineCheck.options` replaces ESLint's `schema` array. Declare
`allowedMethods` with a default; the resolved value lands in `opts.allowedMethods`
as `readonly string[]`, typed without any casting.

### `prefer-const`

ESLint flags `let` declarations that are never reassigned. This rule requires
flow analysis and is type-aware — it belongs in the `requiresTypes: true`
bucket (see §7). A weaker heuristic — flag `let` declarations whose identifier
is never referenced again on the same line — is expressible as a line walk
and doesn't need `ctx.types` at all:

```ts
export default defineCheck({
  id: "PreferConst",
  category: Category.Refactor,
  basePriority: 5,
  explanation: "Use const for declarations that are never reassigned.",
  run(file, ctx) {
    for (const ln of file.lines()) {
      if (ln.isComment) continue;
      if (!ln.isStringLiteral && /^\s*let\s/.test(ln.text)) {
        // Heuristic only: report; suppress via `// cofferdam-ignore: PreferConst`
        // on lines that are legitimately reassigned elsewhere.
        ctx.report({
          message: "Prefer const over let when the binding is not reassigned.",
          span: ln.spanFor(ln.text.indexOf("let"), ln.text.indexOf("let") + 3),
        });
      }
    }
  },
});
```

Document the heuristic limitation in `explanation` or `body`. The `body` field
(shown by `cofferdam explain --full`) is the right place for caveats that would
clutter the one-line message.

### `no-unused-vars`

Tracking whether a binding is used requires scope analysis — also type-aware.
Until ts-morph routing lands (§7), a scoped line-walk can catch the most
obvious single-file case: an identifier declared at the top level and never
referenced again in the file.

```ts
export default defineCheck({
  id: "NoUnusedExports",
  category: Category.Refactor,
  basePriority: 8,
  explanation: "Exported binding is never imported elsewhere in the corpus.",
  // True corpus-wide dead-export detection would use a two-pass
  // corpus check (see cofferdam-checks design.rs::DuplicateExportName for
  // the cross-file pattern). This example flags the single-file heuristic.
  run(file, ctx) {
    if (!file.ast) return;
    for (const node of file.ast.findAll("Function")) {
      if (!node.name) continue;
      const refs = file.ast.findAll("IdentifierReference")
        .filter((r) => r.name === node.name);
      // The declaration itself is one reference; if it is the only one
      // and the function is top-level, flag it.
      if (refs.length <= 1) {
        ctx.report({
          message: `"${node.name}" is declared but never referenced in this file.`,
          span: node.span,
        });
      }
    }
  },
});
```

## Cross-file plugin checks: corpus + finalize (cd-9hp.6)

A plugin that needs to see *all* files before emitting findings —
duplicate exports, orphan modules, layered-import audits — uses two
hooks: the per-file `run` to accumulate state, and a `finalize`
callback that runs once after every per-file pass has completed.

The state itself lives in a per-plugin store called **the plugin
corpus**, exposed on the context as `ctx.corpus`. It is plugin-private
by construction: two plugins picking the same slot key cannot see each
other's data. The host namespaces every key by the calling
`check.id`.

### Quick example

A check that flags duplicate class names across files. The snippet is
imported from the real plugin under `examples-plugins/duplicate-class/`
— inlining it as a fenced TS block trips VitePress' Vue compiler on
the TypeScript generic syntax (`ctx.corpus.append<ClassDecl>(...)`,
`Map<string, ClassDecl[]>`). The snippet import sidesteps that.

<<< @/../examples-plugins/duplicate-class/src/index.ts{ts}

### `ctx.corpus` API

| Method | Purpose |
|---|---|
| `read<T>(key)` | Return the last value written, or `undefined`. |
| `write<T>(key, value)` | Overwrite the slot. `value` must be JSON-serialisable. |
| `append<T>(key, item)` | Get-or-create-array, push `item`. Optimised for the common `Vec<T>` aggregation pattern. |

The slot value lives in memory for the duration of the analysis run.
There is no cross-run persistence — `cofferdam --watch` (cd-9hp.4)
will eventually share corpus state between incremental runs, but
plugins should not rely on that today.

### `finalize(ctx, opts)`

Optional. Same `defineCheck` input as `run`. Invoked once per
analysis run, after every per-file `run` has completed, in plugin
declaration order. The context exposes:

* `ctx.corpus` — the same per-plugin store the per-file pass populated.
* `ctx.report({ file, message, span, related?, severity?, fix? })` —
  emit a finding. Unlike per-file `report`, you must pass an explicit
  `file` because finalize has no implicit "current file". Cross-file
  findings typically attach to one canonical location and use
  `related` for the other implicated files.

**The `file` (and every `related` entry's `file`) must be a path that
was part of the analyzed set** — i.e. one of the paths your `run`
hook saw as `file.path`. The CLI rejects reports for any other path:
the finding is dropped and replaced by one aggregated
`Warning.PluginHostFailed` naming your check id and the offending
paths. An out-of-scope `related` entry is dropped individually while
its finding survives. The reliable pattern is to record `file.path`
into `ctx.corpus` during `run` and read it back in `finalize`, rather
than reconstructing paths by hand.

A `finalize` crash is surfaced as
`Warning.PluginCrashed` with the message `plugin '<id>' threw in
finalize(): …`; the rest of the run is unaffected.

### Why namespace by check id?

Built-in checks ship as one compile-time-reviewed workspace, so two
`CorpusKey<T>` constants colliding on a name with mismatched `T` is
caught at code review and surfaces loudly via a panic. Plugins are
hostile-by-default: two unrelated authors can both pick
`"exports"` and the run would otherwise crash mid-analysis. The
plugin host therefore stores each plugin's slots in its own private
map; from a plugin's perspective, no key conflict with any other
plugin is possible.

The Rust runtime underneath (cd-9hp.7) supports the same model via
`CorpusIndex::try_with_namespaced_slot(check_id, key, …)`. Plugin
authors don't see that API directly — the JS host translates
`ctx.corpus.read/write/append` into the appropriate namespaced calls.

### Contract summary

| Caller | API | Failure mode |
|---|---|---|
| Built-in Rust checks | `corpus.with_slot(&KEY, &#124;t&#124; ...)` | Panic on type mismatch (logic bug) |
| Built-in Rust checks (deliberately fallible) | `corpus.try_with_slot(&KEY, &#124;t&#124; ...)` | Returns `Err(CorpusError::TypeMismatch)` |
| Plugin checks (JS) | `ctx.corpus.read/write/append(key, ...)` | Slots are plugin-private; collision between plugins is impossible by construction |

Genuinely cross-check shared storage (e.g. the IMPORTS / EXPORTS slots
in `cofferdam_core::graph` that several built-ins read in `finalize`)
keeps using raw `with_slot`. The namespacing is intentional isolation
for plugin authors who don't know what other plugins exist.

The ESLint `no-unused-vars` option `{ vars: "all", args: "after-used" }` maps
to your `options` schema:

```ts
options: {
  vars: { default: "all", type: "string" },
  args: { default: "after-used", type: "string" },
},
```

`opts.vars` and `opts.args` are then `string`, validated at startup against the
user's `cofferdam.toml` override.

## 4. Migrating Credo custom checks

Credo checks are Elixir modules that implement `Credo.Check`. They receive a
`SourceFile` with tokenized lines and an AST. The cofferdam SDK maps to the
same concepts in TypeScript.

### Before (Elixir) — BrandCasing

```elixir
defmodule ExamplcoHost.Credo.BrandCasing do
  @moduledoc "Brand name must be all-caps EXAMPLCO in user-facing copy."
  use Credo.Check,
    category: :warning,
    base_priority: :high,
    param_defaults: [brand: "EXAMPLCO", allowed_aliases: []]

  def run(source_file, params \\ []) do
    brand = Params.get(params, :brand, __MODULE__)
    allowed = Params.get(params, :allowed_aliases, __MODULE__)
    issue_meta = IssueMeta.for(source_file, params)

    source_file
    |> SourceFile.lines()
    |> Enum.with_index(1)
    |> Enum.reduce([], fn {line, line_no}, issues ->
      cond do
        String.match?(line, ~r/# brand:ignore/) -> issues
        String.match?(line, ~r/^.*".*Examplco.*"/) ->
          [issue_for(issue_meta, line_no, brand) | issues]
        true -> issues
      end
    end)
  end

  defp issue_for(issue_meta, line_no, brand) do
    format_issue(issue_meta,
      message: "Brand name must be #{brand}, not Examplco.",
      line_no: line_no
    )
  end
end
```

### After (TypeScript) — BrandCasing

```ts
import { defineCheck, Category } from "@cofferdam/check-sdk";

const TRIGGER = /\bExamplco\b/;
const PLUGIN_IGNORE = /\bbrand:ignore\b/;

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  explanation: "Brand name must be all-caps EXAMPLCO in user-facing copy.",
  options: {
    brand: { default: "EXAMPLCO", type: "string" },
    allowedAliases: { default: [] as string[], type: "string[]" },
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
      if (!ln.isStringLiteral && !ln.isJsxText) continue;
      const m = TRIGGER.exec(ln.text);
      if (!m) continue;
      if (opts.allowedAliases.some((a) => ln.text.includes(a))) continue;
      ctx.report({
        message: `Brand name must be "${opts.brand}", not "${m[0]}".`,
        span: ln.spanFor(m.index, m.index + m[0].length),
      });
    }
  },
});
```

**Key translation table:**

| Credo | cofferdam SDK |
|---|---|
| `use Credo.Check, category: :warning` | `category: Category.Warning` |
| `base_priority: :high` | `basePriority: 15` (numeric, -20..20) |
| `param_defaults: [brand: "EXAMPLCO"]` | `options: { brand: { default: "EXAMPLCO", type: "string" } }` |
| `Params.get(params, :brand, __MODULE__)` | `opts.brand` (typed, no cast) |
| `SourceFile.lines() |> Enum.with_index(1)` | `[...file.lines()]` (1-based `lineNo`) |
| `# brand:ignore` magic comment | plugin-level `ignoredNext` set (same semantics) |
| `format_issue(issue_meta, message: ..., line_no: ...)` | `ctx.report({ message, span })` |

The `NoHttpClient` and `TenantIsolation` patterns from examplco-host are
Elixir/Phoenix checks with no literal TypeScript analogue — they are ported as
the closest equivalent pattern (Pattern B and Pattern C respectively) rather
than a line-for-line translation. See §6 of `plugin-sdk-e2e.md` for the
full sibling-fixture designs.

## 5. Testing your plugin

The SDK's `runPlugin` function is the test entry point. It takes a `Check`
(from `defineCheck`) and a `PluginRunInput` (synthetic or real file data) and
returns `PluginReport[]` you can assert on.

The `packages/check-sdk/tests/plugin-host.test.mjs` file is the reference
harness. Tests run via `node --test` (Node.js built-in test runner) against the
compiled `dist/` output.

### Example: positive fixture test

```mjs
// tests/plugin-host.test.mjs (excerpt — model your own tests on this shape)
import { test } from "node:test";
import { strict as assert } from "node:assert";

test("runPlugin emits one report per matched line, with file-absolute spans", async () => {
  const { defineCheck, Category, runPlugin } = await import("../dist/index.js");

  const check = defineCheck({
    id: "BrandCasing",
    category: Category.Warning,
    basePriority: 15,
    explanation: "Brand must be EXAMPLCO.",
    options: { brand: { default: "EXAMPLCO", type: "string" } },
    run(file, ctx, opts) {
      for (const ln of file.lines()) {
        if (ln.isComment || ln.isDocComment || ln.isPragma) continue;
        if (!ln.isStringLiteral && !ln.isJsxText) continue;
        const m = /\bExamplco\b/.exec(ln.text);
        if (!m) continue;
        ctx.report({
          message: `Brand must be ${opts.brand}, not ${m[0]}.`,
          span: ln.spanFor(m.index, m.index + m[0].length),
        });
      }
    },
  });

  // Synthetic file: three lines, one with a flagged string literal.
  const text = "const x = 1;\nconst y = 'Examplco';\nconst z = 2;\n";
  const lineViews = [
    { lineNo: 1, text: "const x = 1;",         isComment: false, isDocComment: false,
      isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 },
    { lineNo: 2, text: "const y = 'Examplco';", isComment: false, isDocComment: false,
      isStringLiteral: true,  isJsxText: false, isPragma: false, lineStart: 13 },
    { lineNo: 3, text: "const z = 2;",         isComment: false, isDocComment: false,
      isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 36 },
  ];

  const reports = runPlugin(check, { path: "synthetic.ts", text, lineViews });

  assert.equal(reports.length, 1);
  assert.equal(reports[0].checkId, "BrandCasing");
  // "Examplco" starts at byte 11 within line 2; line 2 starts at byte 13.
  assert.equal(reports[0].startByte, 24);
  assert.equal(reports[0].endByte, 32);
  // Round-trip: the reported span slices back to the trigger string.
  assert.equal(text.slice(reports[0].startByte, reports[0].endByte), "Examplco");
});
```

**Anatomy of a `PluginRunInput`:**

- `path` — file path string, forwarded to `PluginReport.file`. Use a relative
  path or a fixture-stable absolute path.
- `text` — full file source, UTF-8. Byte offsets in spans index into this.
- `lineViews` — array of `NativeLineView` objects. Each must carry `lineStart`,
  which is the byte offset of the line's first character in `text`. The `buildSourceFile`
  function the host uses internally constructs `spanFor` from this value.

**What to assert:**

- `reports.length` — exact count of expected findings.
- `reports[i].checkId` — matches the `id` you passed to `defineCheck`.
- `reports[i].startByte` / `reports[i].endByte` — round-trip check:
  `text.slice(startByte, endByte)` must equal the literal text you expect to
  flag. This is the span-fidelity contract from `plugin-sdk-e2e.md` §5.
- `reports[i].severity` — when your check uses per-report severity overrides,
  pin them.

**Testing plugin throws:** `runPlugin` catches exceptions and returns a
`Warning.PluginCrashed` report rather than propagating. Assert on that shape:

```mjs
test("plugin crash surfaces as Warning.PluginCrashed", async () => {
  const { defineCheck, Category, runPlugin } = await import("../dist/index.js");

  const check = defineCheck({
    id: "Misbehaving",
    category: Category.Warning,
    basePriority: 10,
    explanation: "x",
    run() { throw new Error("boom"); },
  });

  const reports = runPlugin(check, {
    path: "f.ts",
    text: "x",
    lineViews: [{ lineNo: 1, text: "x", isComment: false, isDocComment: false,
                  isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 }],
  });

  assert.equal(reports[0].checkId, "Warning.PluginCrashed");
  assert.match(reports[0].message, /boom/);
});
```

**Running the tests:** build first, then run the test file:

```sh
cd packages/check-sdk
npx tsc -p .
node --test tests/plugin-host.test.mjs
```

For your own plugin package, mirror the same structure: a `tests/` directory
with `.test.mjs` files, a build step, and `node --test`.

## 6. Publishing

### Naming convention

Publish under `@yourorg/cofferdam-checks-<scope>`, for example:

- `@acme/cofferdam-checks-security`
- `@acme/cofferdam-checks-prisma`
- `@acme/cofferdam-checks-brand`

Single-check packages are fine; grouping related checks under one package
reduces the user's `plugins = [...]` list.

### `package.json`

```json
{
  "name": "@acme/cofferdam-checks-brand",
  "version": "1.0.0",
  "description": "Brand-name enforcement checks for cofferdam.",
  "type": "module",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc -p ."
  },
  "peerDependencies": {
    "@cofferdam/check-sdk": "^0.2.0"
  },
  "devDependencies": {
    "@cofferdam/check-sdk": "0.2.2",
    "typescript": "^5.6.0"
  }
}
```

`"type": "module"` is required: the plugin host loads your compiled entry
point with a dynamic `import()`, which parses a `.js` file as CommonJS
unless the nearest `package.json` declares `"type": "module"` — omitting it
makes an `export default ...` entry point fail to load (recent Node versions
auto-detect and fall back with a warning; Node 16–19 throw a hard
`SyntaxError`, and the plugin host's error message will point back here).
Every example plugin under `examples-plugins/` sets this field; mirror it.

Declare `@cofferdam/check-sdk` as a `peerDependency`, not a `dependency`. The
cofferdam binary ships its own copy; treating it as a peer prevents version
conflicts at load time. For 0.x: the loader enforces major-version compatibility
at load time. Pin both `@cofferdam/check-sdk` and `cofferdam` to the same exact
version in consuming repos until 1.0.

The `tsconfig.json` should emit to `dist/` with `"declaration": true` so
consumers get types:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "Node16",
    "moduleResolution": "Node16",
    "strict": true,
    "outDir": "dist",
    "declaration": true
  },
  "include": ["src"]
}
```

### Wiring in `cofferdam.toml`

End users add the package to their `plugins` array. The path can be a local
directory, a local compiled file, or a `node_modules` path:

```toml
# cofferdam.toml
plugins = [
  "./node_modules/@acme/cofferdam-checks-brand",
  "./local-checks/tenant-isolation",
]

# Override default options for a specific check:
[checks."BrandCasing"]
brand = "ACMECORP"
allowedAliases = ["AcmeCorpClient"]
```

The engine resolves each entry to a module, calls `loadPlugins`, and runs the
resulting checks alongside the built-ins. Options in the `[checks."<id>"]`
block override the `default` values declared in your `options` schema.

## 7. Type-aware checks

`DefineCheckInput` exposes `requiresTypes?: boolean` for checks that need
resolved types, inferred generics, or call signatures. Setting `requiresTypes:
true` routes the check through ts-morph: the plugin host resolves a
`tsconfig.json` (the same discovery `cofferdam check` already does for
built-in type-aware checks) and loads ts-morph in-process, then exposes a
query surface on `ctx.types` inside `run()`.

```ts
export default defineCheck({
  id: "NoNullableReturn",
  category: Category.Warning,
  basePriority: 10,
  explanation: "Function return type should not include null.",
  requiresTypes: true,
  async run(file, ctx) {
    if (!file.ast) return;
    for (const fn of file.ast.findAll("Function")) {
      if (!ctx.types) continue; // no tsconfig / ts-morph unavailable — skip
      const facts = await ctx.types.typeAt(fn.span.start_byte, fn.span.end_byte);
      if (facts?.includesNull) {
        ctx.report({ message: "Return type includes null.", span: fn.span });
      }
    }
  },
});
```

`ctx.types` is `undefined` — not present on the object — when no `tsconfig.json`
was discovered, or ts-morph failed to load. Guard every use with `if
(ctx.types)` rather than assuming presence; a project without ts-morph
installed, or `[engine] type_aware = false` in `cofferdam.toml`, is a normal
configuration, not an error. `ctx.types.typeAt(startByte, endByte)` returns a
`Promise<TypeFacts | null>` — `null` means "resolved fine, no type at that
span" (not an error); `run()` may be declared `async` so it can `await` the
call. `TypeFacts` mirrors the built-in type oracle's shape: `{ text,
isNullable, includesNull, includesUndefined, isAny }`.

When `--fail-on-type-unavailable` is passed to `cofferdam check` and a loaded
plugin check declares `requiresTypes: true` but the type host couldn't be
started, the run exits with code 2 instead of silently skipping type
information — the same gate `--fail-on-type-unavailable` already applies to
built-in type-aware checks.

`DefineCheckInput` also exposes `outputMode?: boolean` (defaulting to
`false`, mirroring `requiresTypes`'s plumbing) — set it to opt a check
into `cofferdam verify --dist`. See [Verifying built output](/verify-dist).

### Resolving imported literals (`ctx.types.resolveLiteral`)

`ctx.types` also exposes `resolveLiteral(startByte, endByte)`, which resolves
an identifier — including one imported from another file — to its literal
value via ts-morph's cross-file symbol resolution. This is the SEO
metadata use case this epic is building toward: a check that inspects a
page component's exported metadata often needs the *value* behind an
imported constant, not just its type.

Given a shared constants file:

```ts
// constants.ts
export const description = "A great page about widgets";
```

...imported and used in a page file:

```ts
// page.ts
import { description } from "./constants";

export const metadata = { description };
```

A check can resolve `description`'s value at its use site in `page.ts`:

```ts
export default defineCheck({
  id: "NonEmptySeoDescription",
  category: Category.Warning,
  basePriority: 10,
  explanation: "SEO description metadata should not be empty.",
  requiresTypes: true,
  async run(file, ctx) {
    if (!file.ast || !ctx.types) return;
    for (const id of file.ast.findAll("IdentifierReference")) {
      if (id.name !== "description") continue;
      const facts = await ctx.types.resolveLiteral(id.span.start_byte, id.span.end_byte);
      if (facts?.literalString !== undefined && facts.literalString.trim() === "") {
        ctx.report({ message: "SEO description resolves to an empty string.", span: id.span });
      }
    }
  },
});
```

`resolveLiteral` follows the import from `page.ts` to `description`'s origin
declaration in `constants.ts` — both files are already part of the resolved
tsconfig's project, so the cross-file lookup is a normal ts-morph symbol
resolution, not a second file read. It returns
`Promise<LiteralFacts | null>`:

- `null` means nothing was resolvable at all — the span isn't an identifier,
  it has no symbol, or the symbol has no declarations.
- Otherwise, `{ literalString?, isNullable, isEmptyObject }`: `literalString`
  is set only when the resolved declaration's initializer is a string (or
  no-substitution template) literal; `isNullable` and `isEmptyObject` are
  reported independently — a resolved declaration with a non-literal
  initializer (a function call, a computed value) still comes back with
  best-effort `isNullable`/`isEmptyObject` facts rather than `null`.

Each `resolveLiteral` call is an in-process ts-morph query — same as
`typeAt`, there's no Rust↔Node round trip per call to batch away; calling it
once per candidate identifier in a loop is the expected usage pattern.

This exact check ships as `SeoNonEmptyDescription` in
[SEO-grade checking](/seo-checking#_4-type-aware-description-resolved-across-an-import),
including the shorthand-property resolution gap you'll hit if the check
matches `{ description }` instead of `description: description`.

## 8. Worked example: `examples-plugins/seo/` (CD-86)

`examples-plugins/seo/` is the flagship end-to-end proof for this epic — one
plugin package (`@cofferdam-fixtures/seo`) composing every SDK surface
documented above into checks a real SEO/accessibility audit would want:

| Check | Pattern | SDK surface |
|---|---|---|
| `SeoMissingMetadataExport` | A (line/text scan) | No AST kind exposes export declarations, so a page missing `export const metadata` / `export function generateMetadata` is caught the same way BrandCasing catches a missing magic comment. |
| `SeoImgMissingAlt` | B (AST `findAll`) | `findAll("JSXElement")` + `JSXAttributeNode` (CD-80) — see [JSX findAll](#jsx-findall-flag-img-with-no-alt) above. |
| `SeoDuplicateTitle` | C (corpus + `finalize`) | `findAll("Element")` over the HTML AstView (CD-84) to read `<title>`/`<link rel="canonical">`, aggregated via `ctx.corpus` the same way as [DuplicateClassName](#quick-example), with a second corpus slot for canonical URLs. `outputMode: true` (CD-85) makes it eligible for `cofferdam verify --dist` against a build's HTML output *in addition to* running under plain `cofferdam check` against any `.html` files in scope — one check serves both the source-adjacent and build-output angle on duplicate titles/canonicals. |
| `SeoNonEmptyDescription` | Type-aware (`ctx.types.resolveLiteral`, CD-82) | The `NonEmptySeoDescription` example above, ported verbatim — `examples-plugins/seo/fixture/empty-description/page.tsx` imports an empty-string constant aliased to `description` so the check's `IdentifierReference` match fires. |

The fixture (`examples-plugins/seo/fixture/`) has one directory per
violation plus a fully-compliant `good/page.tsx`, and three built HTML pages
(`page-a.html`, `page-b.html`, `page-c.html`) exercising `SeoDuplicateTitle`'s
output-mode path. See `examples-plugins/seo/README.md` for the full
violation-by-violation breakdown and how to build + run it locally.

## 9. Real-world case study: porting a grep script to a plugin (CD-69)

The examples above are synthetic. This one is a plugin actually running in
production, in the open-source [projektor](https://github.com/TAJD/projektor)
repo: `IslandApiConvention`, at
[`plugins/island-api/src/index.ts`](https://github.com/TAJD/projektor/blob/main/plugins/island-api/src/index.ts).
It replaced a shell script that grepped island components for two
convention violations — declaring a local `buildHeaders` function instead of
using the project's shared `apiFetch` wrapper, and calling `fetch()` directly
instead of going through that wrapper. Porting it to cofferdam surfaced two
lessons worth knowing before you migrate your own grep-based check.

**Lesson 1 — Pattern A and Pattern B often belong in the same check, not two
checks.** The `buildHeaders` half originally stayed a line scan (Pattern A)
because the `const buildHeaders = (token) => ...` arrow-function form wasn't
visible to `file.ast.findAll(...)` — the SDK's AST surface didn't expose a
`VariableDeclaration` node kind, only `function buildHeaders(...)`. CD-78
closed that gap: `findAll("VariableDeclaration")` now returns one node per
`const`/`let`/`var` statement, and each declarator exposes its `name` and
`init`, so the `const`-bound form is a clean AST match (check
`declarationKind === "const"` and `init?.kind === "ArrowFunctionExpression"` —
see the `no-banned-const` example plugin). The `fetch()` half is a
`findAll("CallExpression")` walk, which *is* also a clean AST match:

```ts
// buildHeaders — line scan (Pattern A), because CD-78 means arrow-function
// declarations aren't visible as an AST node kind yet.
const BUILD_HEADERS_PATTERN = /\b(function|const)\s+buildHeaders\b/;
for (const ln of file.lines()) {
  const m = BUILD_HEADERS_PATTERN.exec(ln.text);
  if (!m) continue;
  ctx.report({ message: "...", span: ln.spanFor(m.index, m.index + m[0].length) });
}

// raw fetch() — AST findAll (Pattern B), including the window.fetch() /
// globalThis.fetch() / self.fetch() bypasses the original grep missed.
for (const call of file.ast.findAll("CallExpression")) {
  const flagged = describeRawFetchCallee(call.callee); // MemberExpression walk
  if (!flagged) continue;
  ctx.report({ message: `Raw "${flagged}" call — ...`, span: call.span });
}
```

When migrating a grep script, don't force every rule in it into one pattern.
Match each half of the check to whichever pattern actually expresses it, the
same way this check keeps a line scan for one violation and an AST walk for
the other.

**Lesson 2 — a trailing `dir/**` glob has a gap; use `dir/**/*`.** Scoping the
check to one directory (`files.pathPatterns: ["apps/web/src/islands/**/*"]`)
hit a real cofferdam bug along the way: a bare trailing `**` (no `/*` suffix)
never matches a file sitting *directly* in the scoped directory — only files
nested at least one directory deeper. `examples-plugins/plugin-scope-glob/`
is the regression fixture for this (filed as CD-70, alongside a related
silent-zero-match bug, CD-71, that made the gap hard to notice in the first
place — a plugin whose `pathPatterns` matches nothing now emits
`Warning.PluginZeroScopeMatch` instead of silently finding zero issues). Until
CD-70 is fixed upstream, scope a plugin to a single directory with a trailing
`/*`, not a bare `**`.

The full plugin, its fixture, and its `expected.json` regression contract are
worth reading end to end if you're about to port your own team's grep/shell
convention check: [`plugins/island-api/`](https://github.com/TAJD/projektor/tree/main/plugins/island-api)
in the projektor repo. There's also a public tutorial walking through it —
[How the island API convention plugin works](https://tajd.github.io/projektor/contributing/island-api-plugin/) —
aimed at projektor contributors but equally useful as a second worked example
of this guide's Pattern A/B split.

**A note on where `SeoNonEmptyDescription` actually resolves types.**
`cofferdam`'s tsconfig discovery for type-aware *plugin* checks walks UP
from the invoking process's current directory only — it never searches
into subdirectories of whatever path `check`/`verify` was pointed at. The
repo-root-anchored `scripts/check-plugin-fixtures.mjs` pipeline that
generates every plugin fixture's `expected.json` therefore never
discovers `examples-plugins/seo/fixture/tsconfig.json`, so that golden
file legitimately shows `Warning.PluginTypeHostUnavailable` rather than a
firing `SeoNonEmptyDescription` finding. The check is still real and
end-to-end tested with a live ts-morph resolution — see
`examples-plugins/seo/test/type-aware-description.test.mjs`, which runs
`cofferdam check` with its working directory set to `fixture/` (mirroring
`scripts/check-type-host-smoke.mjs`'s convention for the built-in
type host) and asserts the cross-file empty-string resolution actually
fires.
