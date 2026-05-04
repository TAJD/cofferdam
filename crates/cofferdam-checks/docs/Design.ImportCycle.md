---
id: Design.ImportCycle
category: Design
default_severity: Medium
base_priority: 8
---

# Design.ImportCycle

Flags circular import chains in the project graph. Cycles cause
initialization-order surprises, complicate refactoring, and usually point
at modules that should be split.

## Why

When file A imports B and B imports A (directly or through any chain),
the runtime has to choose which module is "half-loaded" while resolving
the other. The result is a class of bugs where exports appear `undefined`
at first call, work later, and disappear again under HMR. Static analysis
can find these cleanly — there's no reason to wait for them to bite.

## What gets flagged

* Direct cycles: `a.ts → b.ts → a.ts` (two-file cycle).
* Longer cycles: `a → b → c → … → a` of any length.
* Self-imports: a file importing itself (degenerate but always wrong).

Each cycle produces one finding, anchored at the alphabetically-lowest
member's first import edge into the cycle. Other members appear in the
finding's `related` spans so editors and the SARIF formatter can
highlight every participant.

## What's not flagged

* Cycles that exist only via `import type { … }` edges. TypeScript erases
  type-only imports during compilation, so they cause no runtime cycle.
  Flip `ignore_type_only = false` in config to see them anyway.
* Edges that leave the project graph (anything resolving into
  `node_modules`, or unresolved bare specifiers). The check is
  in-project-only by design.

## Configuration

```toml
[checks."Design.ImportCycle"]
ignore_type_only = true
```

## Limitations

* Dynamic imports (`import('./m')`) participate in the cycle detection —
  they're real edges. If your code uses dynamic imports specifically to
  break a static cycle, you'll still get a finding; suppress with the
  inline directive at the import site.
* `require()` is not tracked, so CommonJS-only cycles slip through.
* Re-export forwarders (`export { x } from './y'`) ARE edges because the
  re-exporter performs the import; if barrel files form a cycle, that
  shows up here.
