---
id: Design.EffectLeakage
category: Design
base_priority: 8
default_severity: Medium
options: []
---

A file annotated `@pure` whose transitive import chain reaches a known side-effecting module (filesystem, network, a database client) — the annotation makes a promise the code doesn't keep.

```ts
// @pure
import { readFile } from "./disk-cache";
// disk-cache.ts does `import * as fs from "fs";` internally — the
// side effect is one hop away, not in this file's own imports.

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

```ts
// fix — drop the tag, or remove the dependency on the side-effecting module
import { readFile } from "./disk-cache";

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Not flagged — no `@pure` tag, so no contract is being made:

```ts
import * as fs from "fs";

export function loadConfig(): string {
  return fs.readFileSync("config.json", "utf8");
}
```

Not flagged — `@pure`, and the import chain never reaches a side-effecting module:

```ts
// @pure
export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Scope: `@pure` is read as a whole-file contract — any comment anywhere in the file containing the literal text `@pure` opts the file in; there's no per-function granularity, since the transitive walk operates on the engine's file-level import graph. The side-effecting module list (Node built-ins like `fs`/`net`/`http`/`child_process`, and common database/queue clients like `pg`, `mongodb`, `redis`, `prisma`) is a fixed denylist, not user-configurable in v1. Only imports that resolve internally are followed transitively; an unresolved (external) specifier is checked against the denylist and ends that branch of the walk either way — so a side-effecting package reached through several internal re-exports is still caught, but a side-effecting package not on the denylist is not.
