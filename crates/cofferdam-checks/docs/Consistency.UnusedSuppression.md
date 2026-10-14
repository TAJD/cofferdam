---
id: Consistency.UnusedSuppression
category: Consistency
base_priority: -5
default_severity: Low
options: []
---

A `cofferdam-ignore` suppression directive names a check ID but no current finding for that check exists in the scope the directive covers. The underlying issue was likely fixed, the code was deleted, or the check was renamed — the directive is now dead weight.

Three forms are checked:

- **Next-line** — `// cofferdam-ignore: <CheckId>[: reason]` where the next non-blank line has no matching finding.
- **Range** — `// cofferdam-ignore-start: <CheckId>` … `// cofferdam-ignore-end` where the covered lines have no matching finding.
- **File-wide** — `// cofferdam-ignore-file: <CheckId>` where the file has no matching finding anywhere.

Broad-form directives (no check id, e.g. `// cofferdam-ignore`) are not flagged here — that's `Consistency.BroadSuppression`'s territory. Directives targeting a check ID not installed in the current engine run are also skipped — those are `Consistency.UnknownCheckId`'s territory.

**Stale (flag):**

```ts
// cofferdam-ignore: Refactor.PurityHeuristic: legacy impurity
export function double(n: number) {
  return n * 2; // pure — no module-level mutable reads — suppression is stale
}
```

**Stale range (flag):**

```ts
// cofferdam-ignore-start: Refactor.LongAndComplex
function simple() {
  return 42;
}
// cofferdam-ignore-end
```

**Still valid (no finding):**

```ts
let counter = 0;
// cofferdam-ignore: Refactor.PurityHeuristic: intentional shared counter
export function next() {
  return counter++;
}
```

Remove stale directives to keep the suppression list auditable and reviewable. A suppression with no finding is noise that erodes trust in suppressions that are still load-bearing.
