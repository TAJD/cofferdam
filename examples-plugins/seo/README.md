# `@cofferdam-fixtures/seo` — SEO/accessibility plugin (CD-86)

The flagship end-to-end integration proof for the CD-79 "SEO-grade checking"
epic. One plugin package, four checks, composing every SDK surface the epic
landed:

- **JSX AstView** (CD-80) — `findAll("JSXElement")` for `<img>` alt-text.
- **Cross-file type resolution** (CD-82) — `ctx.types.resolveLiteral`
  follows an imported constant to its origin declaration.
- **HTML AstView** (CD-83/84) — `findAll("Element")` against real `.html`
  output, plus the `ctx.corpus`/`finalize` cross-file pattern.
- **`verify --dist` output-mode eligibility** (CD-85) — the same
  duplicate-title check also runs against built HTML output.

## The checks

| id | what it flags |
|---|---|
| `SeoMissingMetadataExport` | A Next.js App Router `page.ts`/`page.tsx` with no `export const metadata` and no `export function generateMetadata`. Text/regex heuristic — the frozen plugin AST surface has no `ExportNamedDeclaration` node kind. |
| `SeoImgMissingAlt` | A JSX `<img>` element with no `alt` attribute (and no `{...spread}` that might supply one). |
| `SeoDuplicateTitle` | A `<title>` or `<link rel="canonical">` duplicated across more than one `.html` file. `outputMode: true`, so it also runs under `cofferdam verify --dist`. |
| `SeoNonEmptyDescription` | An identifier named `description` that resolves — via `ctx.types.resolveLiteral`, across an import — to an empty string literal. |

## Build + run

This plugin depends on SDK surface (`outputMode`, `resolveLiteral`, the
HTML AstView) that hasn't been published to npm yet — `@cofferdam/check-sdk`
resolves to `"*"` in `package.json`, so a plain `npm install` will fetch the
last *published* SDK release and silently drop those fields. Build the SDK
locally and copy it into `node_modules` by hand, mirroring
`.github/workflows/plugin-sdk-e2e.yml`'s "Bundle SDK into each plugin's
local node_modules" step:

```bash
# from the repo root
cd packages/check-sdk && npm run build && cd -

cd examples-plugins/seo
npm install        # pulls typescript + ts-morph (ts-morph refetches a stale
                    # check-sdk too — the next two lines fix that)
rm -rf node_modules/@cofferdam/check-sdk/dist
cp -r ../../packages/check-sdk/dist ../../packages/check-sdk/package.json \
  node_modules/@cofferdam/check-sdk/

npm run build      # tsc -p .
cofferdam check fixture --config cofferdam.toml
cofferdam verify --dist fixture --config cofferdam.toml
```

Expected `cofferdam check fixture` output (see `expected.json`):

- `SeoDuplicateTitle` x2 on `fixture/page-a.html` (duplicate `<title>` and
  duplicate canonical URL, both shared with `fixture/page-b.html`).
- `SeoImgMissingAlt` on `fixture/missing-alt/page.tsx`.
- `SeoMissingMetadataExport` on `fixture/no-metadata/page.tsx`.
- A `Warning.PluginTypeHostUnavailable` notice — see "Type-aware checks and
  CWD" below for why `SeoNonEmptyDescription` doesn't fire in this exact
  invocation.
- One built-in `Design.DuplicateExportName` finding (all three
  `metadata`-exporting fixture pages share that export name) — expected
  noise, same as `duplicate-class`'s own golden file.

`fixture/page-c.html` (unique title/canonical), `fixture/good/page.tsx`
(fully compliant), and `constants.ts` (not a page) deliberately produce no
plugin findings.

## Fixture layout

```
fixture/
  constants.ts                 # shared description constants
  no-metadata/page.tsx         # violates SeoMissingMetadataExport only
  missing-alt/page.tsx         # violates SeoImgMissingAlt only
  empty-description/page.tsx   # violates SeoNonEmptyDescription only
  good/page.tsx                # fully compliant
  page-a.html, page-b.html     # duplicate <title>/canonical pair
  page-c.html                  # unique <title>/canonical, no findings
  tsconfig.json                # see "Type-aware checks and CWD" below
```

`page.ts`/`page.tsx` files live one per subdirectory because
`SeoMissingMetadataExport`'s `files.pathPatterns: ["**/page.ts",
"**/page.tsx"]` matches on the literal basename `page.ts`/`page.tsx` — the
Next.js App Router convention.

`expected.json` is the committed golden; regenerate it with `node
scripts/regen-plugin-fixtures.mjs` after a fixture or check change, from the
repo root, and diff-check it with `node scripts/check-plugin-fixtures.mjs`.

## How it works

### The dual-slot HTML corpus check (`SeoDuplicateTitle`)

```ts
// run() — once per .html file
ctx.corpus.append<TitleRecord>("titles", { file: file.path, text, span: el.span });
ctx.corpus.append<CanonicalRecord>("canonicals", { file: file.path, href, span: el.span });

// finalize() — once per analysis run, after every file's run() completes
// groups each slot independently by (text | href), emits one finding per
// cross-file duplicate group in each slot
```

Follows the exact same corpus + finalize pattern as
[`DuplicateClassName`](../duplicate-class/), extended to two independent
slots. `outputMode: true` on the `defineCheck` call is what makes this
check *also* eligible for `cofferdam verify --dist` — no second check
needed; the same logic runs against source-adjacent `.html` files under
`cofferdam check` and against a build's HTML output under `verify --dist`.

### The cross-file literal resolution check (`SeoNonEmptyDescription`)

```ts
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
}
```

`fixture/empty-description/page.tsx` imports `badDescription` (an empty
string in `constants.ts`) aliased to `description`, and uses it as an
**explicit** `description: description` property — not the `{ description
}` shorthand form the SDK guide's own worked example uses. ts-morph's
symbol resolution for a shorthand property assignment resolves to the
property's own symbol rather than following the alias through to the
imported binding, so `resolveLiteral` can't see through it; this is a
pre-existing gap in `resolveLiteral`'s CD-82 implementation
(`type-host-core.mjs`), out of scope for this ticket. The explicit form
sidesteps it while still exercising the same cross-file `resolveLiteral`
resolution the check is built around.

### Type-aware checks and CWD

`cofferdam`'s tsconfig discovery for type-aware plugin checks walks UP
from the invoking process's *current directory* — never into
subdirectories of the path `check`/`verify` was pointed at. Running
`cofferdam check fixture` from the repo root (or from this package's own
directory) therefore never discovers `fixture/tsconfig.json`, so
`SeoNonEmptyDescription` runs with `ctx.types` undefined and the committed
`expected.json` shows `Warning.PluginTypeHostUnavailable` instead of a
firing finding — this is also why `scripts/regen-plugin-fixtures.mjs` /
`scripts/check-plugin-fixtures.mjs` (both invoked from the repo root)
never exercise this check's live resolution path.

To see it actually resolve, run `cofferdam check .` with the working
directory set to `fixture/` itself (mirroring
`scripts/check-type-host-smoke.mjs`'s convention for the built-in
type-aware oracle):

```bash
cd examples-plugins/seo/fixture
../../../target/release/cofferdam check . --config ../cofferdam.toml --only Warning.SeoNonEmptyDescription
```

(ts-morph must be installed — `npm install` in `examples-plugins/seo`,
not in `fixture/` itself; see "Build + run" above.)

`examples-plugins/seo/test/type-aware-description.test.mjs` automates
exactly this and asserts the finding fires.

## Tests

```bash
node --test examples-plugins/seo/test/*.test.mjs
```

- `verify-dist.test.mjs` — asserts `cofferdam verify --dist` finds
  `SeoDuplicateTitle` against the same fixture `cofferdam check` uses, with
  `origin: build_output`, and that `.tsx` files never appear (verify's
  discovery is `.html`/`.htm`-extension-scoped).
- `type-aware-description.test.mjs` — asserts `SeoNonEmptyDescription`'s
  live `resolveLiteral` resolution (see above).
