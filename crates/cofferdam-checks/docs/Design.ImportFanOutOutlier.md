---
id: Design.ImportFanOutOutlier
category: Design
base_priority: 6
default_severity: Medium
options: []
---

A file's import fan-in (files that import it) or fan-out (files it imports) is a statistical outlier versus the rest of the project — a likely "god module" (doing too much) or over-centralized dependency (too many things depend on one module), found without a hardcoded threshold.

```ts
// god.ts — imports a dozen unrelated modules; flagged for fan-out
import { a } from "./a";
import { b } from "./b";
import { c } from "./c";
// ...ten more...
```

```ts
// utils.ts — imported by nearly every other file; flagged for fan-in
export function formatDate() { /* ... */ }
```

Not flagged — `index.ts` and `types.ts` barrels are excluded entirely, since a real aggregator's high fan-in/fan-out is by design, not a smell:

```ts
// index.ts
export * from "./a";
export * from "./b";
export * from "./c";
```

Statistics: computed only over in-project (resolved) import edges — an external package import (`react`, `lodash`) doesn't count toward either metric. `index.*`/`types.*` basenames are excluded from the statistical population entirely (not just from being flagged), since including their legitimately extreme counts would inflate the mean/stddev for every other file. Below 8 non-excluded files in the project, or when a metric's standard deviation is 0 (every file has the same count), nothing is flagged for that metric — there isn't enough variation to call anything an outlier. A file's fan-in (or fan-out) must exceed the project mean plus 3 standard deviations to be flagged; a file can be flagged for both metrics independently.

Scope: v1's hub exclusion is a fixed basename list (`index.ts`/`index.tsx`/`index.js`/`index.jsx`/`index.mjs`/`index.cjs`/`types.ts`/`types.tsx`) — a project with a differently-named central hub (e.g. `container.ts`, a DI container) isn't covered and may be flagged as a false positive.
