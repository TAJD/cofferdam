---
id: Refactor.PreferNullishCoalescing
category: Refactor
base_priority: 3
default_severity: Low
options: []
---

`x || default` falls through on every falsy value (`0`, `""`, `false`). Use `??` to fall through only on `null`/`undefined`. Today the check ships the narrow high-confidence shape: `member-access || default-literal` (string / number / bool / `null` / bare `undefined` / array literal / object literal). Bare-identifier LHS, function-call LHS, and arithmetic LHS are deliberately not flagged — they're too often genuine alt-branch logic without type info. The rule broadens once the type-aware tier lands.

```ts
// flagged
const timeout = config.timeout || 5000;     // 0 should NOT fall through
const name = user.name || "anonymous";      // "" should NOT fall through
```

```ts
// fix
const timeout = config.timeout ?? 5000;
const name = user.name ?? "anonymous";
```

```ts
// not flagged (intentional falsy fallthrough)
const flag = isAdmin || isOwner;            // bare identifier — alt branch
const value = compute() || 0;              // call result — return type ambiguous
const sum = (a + b) || 1;                  // arithmetic — clearly wants falsy
```
