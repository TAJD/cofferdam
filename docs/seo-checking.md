# SEO-grade checking

A worked, end-to-end walkthrough of building an SEO/accessibility plugin
with cofferdam — from a plain text-scan check through cross-file HTML
duplicate detection and type-aware literal resolution, finishing with
verifying a real build's HTML output. Every example here is lifted
directly from [`examples-plugins/seo/`](https://github.com/TAJD/cofferdam/tree/main/examples-plugins/seo),
a real, tested plugin package you can read, run, and copy from.

This page assembles four SDK surfaces that each have their own reference
documentation — [Author guide](/plugin-sdk-guide), [Type-aware checks](/type-aware-checks),
and [Verifying built output](/verify-dist) — into one continuous use case.
If you only need the API reference for one surface, those pages are more
direct; this page is for readers who want to see how the pieces fit
together.

## The use case

A Next.js-style site (or any framework emitting one component per page)
wants CI to catch four classes of SEO/accessibility regression:

1. A page shipped with no `metadata` export — no title, no description.
2. An `<img>` with no `alt` text.
3. Two pages accidentally sharing the same `<title>` or canonical URL.
4. A `description` that resolves to an empty string, even when it's set
   via an imported constant rather than a literal in the page file.

Each maps to a different pattern from the [Author guide](/plugin-sdk-guide),
composed here into one plugin package with four checks.

## 1. Text-scan check: missing metadata export

The frozen plugin AST surface has no `ExportNamedDeclaration` node kind
(see [Pattern A](/plugin-sdk-guide#pattern-a-—-line-walk-with-magic-comment-exemption)),
so `SeoMissingMetadataExport` falls back to a regex scan over `file.text`
for `export const metadata` / `export function generateMetadata`:

```ts
// examples-plugins/seo/src/missing-metadata-export.ts
export default defineCheck({
  id: "SeoMissingMetadataExport",
  category: Category.Warning,
  basePriority: 12,
  explanation: "A page component has no `metadata` or `generateMetadata` export, so it has no page title/description for search engines or social previews.",
  files: { extensions: ["ts", "tsx"], pathPatterns: ["**/page.ts", "**/page.tsx"] },
  run(file, ctx) {
    if (HAS_METADATA.test(file.text) || HAS_GENERATE_METADATA.test(file.text)) return;
    ctx.report({ message: "Page component has no `metadata` or `generateMetadata` export.", span: { ... } });
  },
});
```

`pathPatterns: ["**/page.ts", "**/page.tsx"]` scopes the check to the
Next.js App Router convention — one `page.tsx` per route directory — so it
never fires on a shared component file.

## 2. AST `findAll`: `<img>` missing `alt`

This is the [JSX findAll example](/plugin-sdk-guide#jsx-findall-—-flag-img-with-no-alt)
from the Author guide, verbatim — `findAll("JSXElement")` (CD-80), treating
a `{...spread}` attribute as "can't tell, don't flag":

```ts
// examples-plugins/seo/src/img-missing-alt.ts
for (const el of file.ast.findAll("JSXElement")) {
  if (el.tagName !== "img") continue;
  const hasAlt = el.attributes.some((a) => a.isSpread || a.name === "alt");
  if (!hasAlt) ctx.report({ message: "<img> is missing an `alt` attribute", span: el.span });
}
```

## 3. Cross-file HTML: duplicate `<title>` / canonical URL

`SeoDuplicateTitle` follows the [`ctx.corpus`/`finalize` pattern](/plugin-sdk-guide#quick-example)
against the HTML AstView (CD-83/84) instead of TypeScript source. It reads
every `.html` file's `<title>` text and `<link rel="canonical" href="...">`
via `findAll("Element")`, appends each into its own corpus slot during
`run()`, then groups both slots by value and emits one finding per
cross-file duplicate group in `finalize()`:

```ts
// examples-plugins/seo/src/duplicate-title.ts
run(file, ctx) {
  for (const el of file.ast.findAll("Element")) {
    if (el.tagName === "title") {
      ctx.corpus.append("titles", { file: file.path, text: elementText(el), span: el.span });
    }
    if (el.tagName === "link" && attr(el, "rel") === "canonical") {
      ctx.corpus.append("canonicals", { file: file.path, href: attr(el, "href"), span: el.span });
    }
  }
},
finalize(ctx) {
  reportDuplicates(ctx, "titles", (text) => `<title>"${text}" is duplicated across ${n} files.`);
  reportDuplicates(ctx, "canonicals", (href) => `canonical URL "${href}" is duplicated across ${n} files.`);
},
```

Two independent corpus slots (`titles`, `canonicals`) sharing one
`finalize` helper, rather than two separate checks — see
["The dual-slot HTML corpus check"](https://github.com/TAJD/cofferdam/blob/main/examples-plugins/seo/README.md#the-dual-slot-html-corpus-check-seoduplicatetitle)
in the plugin's own README for the full listing.

## 4. Type-aware: description resolved across an import

Most checks that inspect `metadata.description` only see a literal string
sitting right there in the page file. Real pages usually pull it from a
shared constants file instead — `SeoNonEmptyDescription` needs to follow
that import to know whether the *resolved* value is empty. This is exactly
what [`ctx.types.resolveLiteral`](/plugin-sdk-guide#resolving-imported-literals-ctx-types-resolveliteral)
(CD-82) is for:

```ts
// examples-plugins/seo/src/non-empty-description.ts
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
```

`examples-plugins/seo/fixture/empty-description/page.tsx` imports
`badDescription` (an empty string in `constants.ts`) aliased to
`description`, and assigns it via the **explicit** `description:
description` property form — not the `{ description }` shorthand. ts-morph's
symbol resolution for a shorthand property assignment resolves to the
property's own symbol rather than following the alias through to the
imported binding, so `resolveLiteral` can't see through it today; this is a
pre-existing gap in `resolveLiteral`'s implementation, not something this
check works around. The explicit form sidesteps it while still exercising
genuine cross-file resolution.

::: tip Type-aware plugin checks need a discoverable tsconfig
`resolveLiteral` only resolves when a `tsconfig.json` is discoverable —
cofferdam's tsconfig search for type-aware *plugin* checks walks up from
the invoking process's **current directory**, not from the path being
checked. Running `cofferdam check examples-plugins/seo/fixture` from the
repo root therefore won't discover `fixture/tsconfig.json`, and this check
runs with `ctx.types` undefined — which is why the plugin's own committed
`expected.json` golden shows `Warning.PluginTypeHostUnavailable` instead of
a firing finding. See
[`examples-plugins/seo/test/type-aware-description.test.mjs`](https://github.com/TAJD/cofferdam/blob/main/examples-plugins/seo/test/type-aware-description.test.mjs)
for a test that runs with `cwd` set to `fixture/` and asserts the finding
actually fires.
:::

## 5. Verifying the build's HTML output

Everything above runs against source-adjacent files under `cofferdam
check`. But duplicate titles and canonical URLs are just as easy to
introduce after templating/SSG output — a build step that silently emits
the same `<title>` on two routes. `SeoDuplicateTitle` sets `outputMode:
true`, opting the *same check* into [`cofferdam verify --dist`](/verify-dist)
(CD-85) as well, with no second check needed:

```ts
export default defineCheck({
  id: "SeoDuplicateTitle",
  // ...
  outputMode: true,
  files: { extensions: ["html"] },
  // same run()/finalize() as above
});
```

```sh
cofferdam verify --dist dist/
```

`outputMode: true` only *adds* eligibility for `verify --dist` — it does
not remove the check from the default `cofferdam check` path. One check,
two entry points: source-adjacent `.html` files under `check`, and a
build's rendered output under `verify --dist`.
[`examples-plugins/seo/test/verify-dist.test.mjs`](https://github.com/TAJD/cofferdam/blob/main/examples-plugins/seo/test/verify-dist.test.mjs)
runs `verify --dist` against the same fixture `cofferdam check` uses and
asserts both duplicate findings come back labeled `origin: build_output`.

## Running the full example

```bash
cd packages/check-sdk && npm run build && cd -
cd examples-plugins/seo
npm install
rm -rf node_modules/@cofferdam/check-sdk/dist
cp -r ../../packages/check-sdk/dist ../../packages/check-sdk/package.json node_modules/@cofferdam/check-sdk/
npm run build
cofferdam check fixture --config cofferdam.toml
cofferdam verify --dist fixture --config cofferdam.toml
```

See [`examples-plugins/seo/README.md`](https://github.com/TAJD/cofferdam/blob/main/examples-plugins/seo/README.md)
for the full fixture layout, the exact expected findings, and the test
suite (`node --test examples-plugins/seo/test/*.test.mjs`).
