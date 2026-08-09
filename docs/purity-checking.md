# Purity checking

`Design.EffectLeakage` is cofferdam's one nod to functional-programming
discipline in an otherwise imperative-friendly rule set: it lets you mark
a function or a whole file `@pure`, and then holds that promise to
account by walking the import graph outward until it either runs out of
imports or lands on something that unmistakably touches the outside
world — the filesystem, the network, a database client. This page is the
conceptual walkthrough; the [check reference](/checks/Design.EffectLeakage)
is the terse contract (id, severity, options).

## Why annotate purity at all

Nothing in TypeScript's type system distinguishes a function that
computes a value from one that reads a file, opens a socket, or writes to
a database — `(items: number[]) => number` is a valid signature for both
`sum` and `writeAuditLogAndReturnCount`. Teams that care about this
distinction usually fall back to convention: a comment, a naming scheme,
a review habit of asking "does this touch I/O?" `@pure` turns that
convention into something greppable and, more usefully, something a
linter can hold you to. It's not a compiler-enforced guarantee — nothing
short of an effect system is — but a `@pure` tag that turns out to be
false is exactly the kind of thing a human reviewer would otherwise have
to notice by tracing imports by hand.

```ts
// @pure
export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Tag it, and the check follows every import this function's contract
depends on. If none of them resolve to a known side-effecting module, the
tag is upheld and nothing fires. If one does — even several hops away —
the check flags the tag itself, at the point the promise was made:

```ts
// @pure
import { readFile } from "./disk-cache";
// disk-cache.ts does `import * as fs from "fs";` internally — the side
// effect is one hop away, not in this file's own imports.

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

## Two scopes: file header vs. one function

Early versions of this check only understood a `@pure` comment as a
whole-file contract — wherever the tag appeared, every import anywhere in
the file fed the transitive walk. That is still exactly how it behaves
when the tag is a genuine file header (the comment attached to the file's
very first statement): every example above is a file-header tag, and the
whole file's imports are in scope.

The gap that motivated the newer, narrower scope: a `@pure` comment
placed immediately above one function in a multi-function file used to
opt the *entire file's* imports into the walk, including imports used
only by a completely unrelated function:

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

`computeTotal` never touches `fs` — `loadConfig` does, and it isn't
`computeTotal`'s problem. The check now tells the two cases apart by
where the comment attaches (oxc's parser records, for every comment,
"the start of the token this comment leads"): a comment attached to a
*later*, specific function-like declaration (a function declaration, or a
single-declarator `const`/`let` bound to an arrow or function expression,
including `export default`) scopes the check to just that function. Only
import specifiers actually referenced by an identifier somewhere inside
that function's own body seed the walk — the file's other imports don't
count against it:

```ts
import * as fs from "fs";

export function readConfigFile(): string {
  return fs.readFileSync("config.json", "utf8");
}

// @pure
export function sumItems(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Not flagged: `sumItems` never references `fs`, so `readConfigFile`'s
import doesn't leak onto it. Change the tagged function to use `fs`
directly and it's flagged exactly as you'd expect — the walk starts from
that function's own referenced imports, not the file's.

One deliberate limit: usage-tracking only applies to the tagged function's
*own* body. Once the walk crosses into a different file through a
resolved import, that downstream file's imports are followed as a whole,
same as the old whole-file behaviour — a `@pure` function that imports one
helper from a file which itself imports ten other things, nine of them
unrelated, still gets charged for whichever one of those ten is
side-effecting. Extending usage-tracking past the first hop would mean
re-deriving "is this downstream export actually reachable from that one
call," which is a much harder, whole-program analysis this check doesn't
attempt.

A `@pure` tag that's neither a file header nor attached to a
function-like declaration — a class, a multi-declarator `const`, a
comment floating with nothing recognizable after it — has no effect. That's
an accepted no-op, not a guess: the check would rather say nothing than
attribute a purity contract to the wrong scope.

## The denylist, and extending it

"Side-effecting" is, under the hood, a fixed list: Node built-ins
(`fs`, `net`, `http`, `child_process`, ...) and common database/queue
clients (`pg`, `mongodb`, `redis`, `prisma`, ...). Matching strips a
`node:` prefix (so `node:fs` and `fs` are equivalent) and also matches a
subpath of a listed module — `mysql2/promise` matches the listed
`mysql2`, `@prisma/client/runtime` matches `@prisma/client`.

Every project has its own side-effecting dependencies the built-in list
can't anticipate — an internal HTTP client, a proprietary queue SDK, a
telemetry package. The `extra_side_effect_modules` option adds to the
denylist without replacing it:

```toml
[checks."Design.EffectLeakage"]
extra_side_effect_modules = ["@acme/http-client", "@acme/event-bus"]
```

Matching for the extra list works the same way as the built-in one —
exact specifier or subpath — so listing `@acme/http-client` also catches
`@acme/http-client/retry`.

## What the BFS reports

The walk stops at the *first* side-effecting module it reaches — nearest
by import distance, not every reachable one. If a file (or function) has
two independent paths to a side effect, only one shows up in the message.
This mirrors the check's purpose: it's confirming the promise is broken,
not cataloguing every way it's broken.

Only imports that resolve to a file inside the project are followed
transitively. An unresolved (external) specifier is checked against the
denylist and ends that branch of the walk either way — a side-effecting
package reached through several internal re-exports is still caught, but
a side-effecting package that isn't on the denylist (built-in or
user-supplied) simply isn't seen as one.
