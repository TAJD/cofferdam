---
id: Design.EffectLeakage
category: Design
base_priority: 8
default_severity: Medium
options:
  - name: extra_side_effect_modules
    kind: string_list
    default: []
---

A file (or a specific function within one) annotated `@pure` whose transitive import chain reaches a known side-effecting module (filesystem, network, a database client) — the annotation makes a promise the code doesn't keep.

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

Not flagged — the `@pure` tag documents `computeTotal` specifically; `fs` is only referenced inside the unrelated `loadConfig`, so it isn't attributed to `computeTotal`'s own contract:

```ts
import * as fs from "fs";

export function loadConfig(): string {
  return fs.readFileSync("config.json", "utf8");
}

// @pure
export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Scope: a `@pure` tag is read as a whole-file contract only when the comment is attached to the file's very first statement (a genuine file-header comment, as in every example above except the last) — in that case every import anywhere in the file feeds the transitive walk, same as before CD-138. A `@pure` tag attached instead to a specific, later function declaration or `const`/`let` single-declarator arrow/function assignment scopes the check to that function alone: only import specifiers actually referenced by an identifier inside that function's own body seed the walk. Once the walk crosses into another file, that downstream file's own imports are followed as a whole — usage isn't tracked past the first hop. A `@pure` tag that's neither a file header nor attached to a function-like declaration (e.g. attached to a class, or to a multi-declarator `const` statement) has no effect at all — an accepted no-op rather than a guess. The tag must be on its own line; `@pure` sharing a line with other prose (`/** Does foo. @pure */`) isn't recognized.

The side-effecting module list (Node built-ins like `fs`/`net`/`http`/`child_process`, and common database/queue clients like `pg`, `mongodb`, `mongoose`, `redis`, `prisma`) is a fixed denylist, extendable via `extra_side_effect_modules`. Matching (for both the built-in list and `extra_side_effect_modules`) allows a subpath of a listed module — `mysql2/promise` matches the listed `mysql2` — as well as a `node:`-prefixed specifier. Only imports that resolve internally are followed transitively; an unresolved (external) specifier is checked against the denylist and ends that branch of the walk either way — so a side-effecting package reached through several internal re-exports is still caught, but a side-effecting package not on the denylist (built-in or user-supplied) is not. The BFS reports only the first side-effecting module it reaches (nearest by import distance), not every reachable one, if a file (or function) has more than one path to a side effect.
