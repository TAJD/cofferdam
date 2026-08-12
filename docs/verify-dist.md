# Verifying built output (`cofferdam verify --dist`)

`cofferdam verify --dist <dir>` is an opt-in check mode for **built HTML
output** — static sites, email templates, or any `dist/`, `.next/`,
`build/` tree your build step already produced. It is deliberately a
separate subcommand from `cofferdam check`, not a flag:

- `verify --dist` discovers only the directory you name. It never merges
  into `check`'s discovery roots, so a plain `cofferdam check` run is
  completely unaffected by (and blind to) any dist tree in your repo —
  even one that's gitignored, since `verify` ignores `.gitignore` for the
  directory you explicitly pointed it at.
- `verify --dist` runs **only** checks explicitly tagged as eligible for
  output mode. Source-level and output-level SEO/HTML checks are
  complementary, and a single check may be tagged eligible for both —
  but nothing runs against a `--dist` tree unless it opted in.

## Tagging a check as output-mode

- **Built-in checks** override `Check::output_mode()` to return `true`.
  Defaults to `false`, so every existing check is excluded from `verify`
  runs unless it opts in explicitly. `Html.MissingLangAttribute` is the
  first built-in tagged this way — a build-output page missing `lang` is
  exactly the kind of finding `verify --dist` should catch.
- **Plugin checks** set `outputMode: true` in `defineCheck({ ... })`
  (`@cofferdam/check-sdk`). Defaults to `false`, mirroring
  `requiresTypes`'s plumbing end to end.

## Usage

```sh
cofferdam verify --dist dist/
```

Findings are labelled with their build-output provenance in both output
formats: JSON wraps findings in `{"origin": "build_output", "dist": "...",
"findings": [...]}`, and text output prints a banner line noting `origin:
build_output` before the findings.

## v1 limitations

- **No CSR/SPA support.** `verify --dist` reads the HTML as written to
  disk; it does not run a headless browser, so content injected
  client-side (React/Vue/etc. hydration) is invisible to it. Apps with no
  static HTML output are out of scope for v1 — supporting them would
  require a headless-render step, which isn't implemented.
- **No source-mapping.** Findings report the location in the built HTML
  file, not the original template/component that produced it. Mapping
  build-output findings back to source is build-tool-specific (varies per
  SSG/framework) and isn't attempted — the reported location is always in
  the built HTML.
