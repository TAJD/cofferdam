# `ScopeGlobProbe` — `files.pathPatterns` trailing-`**` regression fixture (CD-70 / CD-71)

Regression-tests two `crates/cofferdam-cli/scripts/plugin-host.mjs` bugs
found while porting a real project's grep-based convention check to a
cofferdam plugin (CD-69):

- **CD-70**: a plugin scoped with a trailing bare `dir/**` pattern (no
  `/*` suffix) never matched a file directly inside `dir/` — only files
  nested at least one directory deeper. `globMatchSingle` compiled
  trailing `**` to `(?:.+/)?`, which requires either zero characters or
  one-or-more full segments each ending in `/`, and a bare filename has
  no trailing `/` to satisfy either branch.
- **CD-71**: a plugin whose `files.pathPatterns` matches zero of the
  discovered files fails silently — indistinguishable from "ran fine,
  found nothing". The host now emits `Warning.PluginZeroScopeMatch`
  when a check's include patterns match none of the files it saw.

## Build + run

```bash
cd examples-plugins/plugin-scope-glob
npm install        # resolves @cofferdam/check-sdk from the workspace
npm run build      # tsc -p .
cofferdam check fixture
```

Expected output: `Warning.ScopeGlobProbe` fires on `fixture/widgets/direct.ts`
(direct child of the scoped `widgets/**` directory) and
`fixture/widgets/nested/deep.ts` (nested child), but not on
`fixture/other/skip.ts` (outside the scope) — despite all three files
carrying the same `SCOPE-PROBE` marker.
