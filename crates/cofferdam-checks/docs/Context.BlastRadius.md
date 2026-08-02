---
id: Context.BlastRadius
category: Context
default_severity: Info
base_priority: 0
---

# Context.BlastRadius

Surfaces the files a change is likely to affect: direct importers of a
changed file, files that call a changed exported symbol by name, files
that reach a changed node transitively (through a bounded-depth walk
of the canonical import graph), and test files that reach the change.

## Why

An edit that compiles cleanly can still break a caller three files
away. `cofferdam check` never sees that caller — it analyzes each file
independently. `Context.BlastRadius` runs the canonical cross-file
graph's bounded-depth traversal from every changed file and reports
who is downstream, ranked so the most directly affected files surface
first.

## What gets surfaced

* **Direct callers of a changed exported symbol** — a file that
  imports a specific named/default export whose declaration span
  overlaps the diff (or, when no line ranges are known, any real
  export of a changed file). Ranked highest.
* **Direct importers** — a file that imports a changed file at all,
  even when the specific imported name isn't known to have changed.
* **Transitive importers** — reached through one or more intermediate
  files, up to a bounded depth. Score decays with distance.
* **Test files reaching the change** — any reached file whose path
  matches common test-file naming conventions, called out explicitly
  so "is this covered" is visible without opening the file.

Every item's `explain` field records the edge path used to reach it,
e.g. `"a.ts imports b.ts imports changed c.ts"`.

## What's not surfaced

* Files with no import-graph path to a changed file within the depth
  bound.
* Bare/unresolved specifiers (`react`, `lodash`) — the canonical graph
  only tracks in-project files.
* Runtime call graphs — only static `import`/`export` edges are
  walked; `require()` and dynamic `import()` are invisible to this
  provider, same limitation as the checks it reuses the graph from.

## Determinism

Item ordering is a pure function of the canonical graph and the
`ChangeSet`: BFS visits nodes in a stable order, ties are broken by
file path, and the digest assembly pipeline breaks any remaining ties
on `(check_id, title)`. No randomness.
