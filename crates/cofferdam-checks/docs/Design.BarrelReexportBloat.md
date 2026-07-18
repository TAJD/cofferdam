---
id: Design.BarrelReexportBloat
category: Design
base_priority: 5
default_severity: Medium
options: []
---

A barrel file (`export * from "./x"` / `export { a, b } from "./x"`) re-exports an unusually large fraction of its directory's exports versus other barrels in the project — the module's real public surface becomes unclear and tree-shaking is defeated, found without a hardcoded threshold.

```ts
// mega-barrel/index.ts — re-exports nearly everything in the directory
export * from "./a";
export * from "./b";
export * from "./c";
// ...ten more, while other barrels in the project re-export a handful
```

Not flagged — a barrel that resolves to the project's (or a workspace package's) published entry point:

```json
// package.json
{ "main": "./index.ts" }
```

```ts
// index.ts — high re-export ratio is intentional here, it's the public API
export * from "./a";
export * from "./b";
```

Statistics: for every file with at least one re-export, the check computes its re-export count as a fraction of its sibling files' real (non-re-export) exports in the same directory, then flags a barrel whose ratio is more than 3 standard deviations above the mean ratio of all other barrels in the project. Below 5 barrel candidates in the project, or when the ratios' standard deviation is 0 (every barrel has the same ratio), nothing is flagged — there isn't enough variation to call anything an outlier.

Entry-point exclusion: a barrel is excluded entirely (from both flagging and the statistical population) when it resolves as a `package.json`'s published entry point. The check walks up from the barrel file to the nearest `package.json` and reads `main`, `module`, `types`/`typings`, and every string reachable inside `exports`, resolving each relative to the `package.json`'s directory and comparing with file extensions stripped (so `"main": "./index.js"` matches a same-directory `index.ts`).

Scope: entry-point matching only catches `package.json` fields that point directly at the source file (or a same-directory build artifact) — a project with a separate `src`/`dist` split whose `main` points into `dist/` isn't covered and the barrel may be flagged as a false positive. A `types`/`typings` field pointing at a `.d.ts` declaration file won't match a barrel's `.ts` source either way, since only the last extension is stripped for comparison — a non-issue since that field wasn't expected to match the analyzed source regardless. A directory whose only files are barrels re-exporting entire subdirectories (no local sibling file with a "real" export of its own) has no sibling denominator and is skipped — arguably the most extreme mega-barrel shape goes undetected, though it's often also the package's own entry point (already excluded above).
