---
id: Readability.MaxFunctionLength
category: Readability
base_priority: -5
default_severity: Low
options: [limit]
---

Functions longer than the configured limit are hard to follow. Break them into smaller helpers. The metric counts lines in the function body, not the whole declaration.

```ts
// flagged: body > 50 lines
function processOrder(order: Order) {
  // ... 80 lines of validation, pricing, side effects, and persistence
}
```

```ts
// fix: extract pure helpers with single responsibilities
function processOrder(order: Order) {
  const validated = validateOrder(order);
  const priced = priceOrder(validated);
  return persistOrder(priced);
}
```

```toml
[checks."Readability.MaxFunctionLength"]
limit = 80
severity = "low"
```
