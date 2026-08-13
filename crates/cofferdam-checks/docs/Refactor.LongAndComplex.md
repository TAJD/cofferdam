---
id: Refactor.LongAndComplex
category: Refactor
base_priority: 12
default_severity: High
options: [length_limit, cyclomatic_limit]
---

Flag functions that are **both** long and cyclomatically complex. Length alone catches flat config blocks and long switch tables that may be perfectly readable; complexity alone catches deeply branching helpers that may still fit on one screen. Functions that exceed both thresholds are the strongest refactor candidates.

Length is measured as body lines excluding blanks and pure-comment lines. Cyclomatic complexity is measured as McCabe count starting at `1`, adding `1` per branching node.

Defaults: `length_limit = 75`, `cyclomatic_limit = 15`. These are intentionally above the standalone limits — this check is about the worst offenders, not the long tail.

```ts
// flagged: 90 effective lines AND cyclomatic 17 — both dimensions exceeded
function processOrder(order: Order, config: Config, env: Env) {
  // ... 90 lines of validation, branching, side effects ...
}
```

```ts
// fix: split along the seams the complexity exposes
function processOrder(order: Order, config: Config, env: Env) {
  const validated = validateOrder(order, config);
  const priced = priceOrder(validated, env);
  return persistOrder(priced);
}
```

```toml
[checks."Refactor.LongAndComplex"]
length_limit = 100
cyclomatic_limit = 20
severity = "high"
```
