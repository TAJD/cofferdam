---
id: Design.LayerViolation
category: Design
default_severity: High
base_priority: 9
---

# Design.LayerViolation

Enforces architectural layering rules declared in
`cofferdam.invariants.toml` (see [the invariants reference](../invariants.md)
for the canonical config location and field reference). Each file is
mapped to a layer via gitignore-style globs, and every import edge is
checked against an explicit allow-list of cross-layer dependencies.

## Why

Layered architectures (hexagonal, onion, clean, n-tier) are easier to
reason about and refactor when the dependency direction is enforced
mechanically. Without enforcement, an `app/` file imports a `domain/`
helper which imports back into `app/` "just this once," and within a
quarter the layers are sand. Cofferdam can hold the line at PR time.

## Configuration

In `cofferdam.invariants.toml` (the legacy single-file `cofferdam.toml`
form is deprecated — see `docs/invariants.md`):

```toml
[layers]
infra  = ["src/infra/**"]
domain = ["src/domain/**", "src/shared/**"]
app    = ["src/app/**"]

[layers.allow]
domain = ["infra"]              # domain may import from infra
app    = ["domain", "infra"]    # app may import from both
# infra omitted → infra is isolated, must not import from any other layer
```

Glob patterns follow gitignore syntax. They're matched against each
file's path relative to the project root (where
`cofferdam.invariants.toml` lives). When multiple layers match a file,
the one with the most-specific glob (longest non-glob prefix in its
include patterns) wins; alphabetical layer name breaks true ties. Use
`!pattern` within a layer's glob list to carve out subtrees explicitly:

```toml
[layers]
ui         = ["components/ui/**"]
components = ["components/**", "!components/ui/**"]
```

## What gets flagged

Every static or dynamic import whose source file is in layer **A** and
whose resolved target is in layer **B**, where **B** is not in the
`allow` list for **A**.

## What's not flagged

* Files outside any declared layer — the project hasn't said how to
  think about them yet, so the check stays silent.
* Same-layer imports — always permitted.
* Type-only imports (`import type { … }`) — they're erased at compile
  time, no runtime layering implication.
* External imports (anything in `node_modules`) — out of scope.

## Limitations

* No "test layer" sugar yet. If you want tests to import from anywhere,
  add a `test` layer to your config and put it on every other layer's
  `allow` list (or, simpler, turn this one check off over the test glob
  with an `[[overrides]]` block carrying `disabled = true`). Do not reach
  for `.cofferdamignore` here: it prunes the files before discovery, so
  they contribute no import edges either, and `Design.OrphanExport` then
  reports every symbol only the tests import as never imported at all
  (CD-325).
* Re-exports through barrel files attribute the violation to the
  re-exporter, not the eventual consumer. If you want barrels to be
  transparent, file an issue — the graph already records re-export
  source paths so attribution can chain.
