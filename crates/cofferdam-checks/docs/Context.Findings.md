---
id: Context.Findings
category: Context
base_priority: 0
default_severity: Info
options: []
---

Advisory `cofferdam context` provider — never runs under `cofferdam check`, never emits an `Issue`.

Summarizes the ordinary check findings that land on files in the current changeset:

- **Fresh findings** — findings on a line the diff actually changed — are rolled into one digest item, grouped and counted by check id (`2× Refactor.PreferConstOverLet, 1× Warning.NoConsoleLog`), pointing to `cofferdam check` for full detail.
- **Legacy debt** — findings elsewhere in a changed file, on lines the diff didn't touch — are rolled into one line per file (`carries N baselined findings`). Individual legacy findings are never listed; the point is visibility without noise.

Both kinds of item are emitted with `pinned: true`, so they always survive digest budget truncation.

```ts
// diff touches only the body of `total`
function total(a, b) {
  if (a == b) {
    // fresh: Warning.TripleEquals
    return a;
  }
  return a + b;
}
```

A file with no line-range information (explicit file list, no git diff resolved) is treated as "whole file changed" — every finding in it is fresh.
