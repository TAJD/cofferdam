---
id: Refactor.MixedThrowAndReturnError
category: Refactor
base_priority: -5
default_severity: Low
options: []
---

A function that both `throw`s and `return`s an error-shaped object literal (a field named exactly `error`, `ok`, or `success`) for what looks like the same class of failure mixes two error-handling idioms. Callers can't compose the error path — some failures need a `try`/`catch`, others need an `if (result.error)` check.

```ts
function parseConfig(input: string) {
  if (!input) {
    throw new Error("input required");
  }
  const parsed = tryParse(input);
  if (!parsed) {
    return { error: "invalid config" };
  }
  return parsed;
}
```

```ts
// fix — pick one idiom
function parseConfig(input: string) {
  if (!input) {
    return { error: "input required" };
  }
  const parsed = tryParse(input);
  if (!parsed) {
    return { error: "invalid config" };
  }
  return { ok: true, value: parsed };
}
```

Not flagged — a function that only ever returns an error-shaped object, with no throw anywhere:

```ts
function loadResult(): Result {
  return { error: null, value: 42 };
}
```

Not flagged — a throw and an error-shaped return in the exact same block (the return is unreachable dead code, not a competing idiom):

```ts
function overlapping(x: number) {
  if (x < 0) {
    throw new Error("negative");
    return { error: "unreachable" };
  }
  return x * 2;
}
```

Scope: only inspects a function's own statements — nested functions (declarations, expressions, arrow functions) are separate scopes and are analyzed independently, not folded into the outer function's throw/return count. Arrow functions themselves aren't currently inspected as the outer subject of this check, only `function` declarations/expressions and class methods.
