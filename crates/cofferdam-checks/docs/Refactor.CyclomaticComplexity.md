---
id: Refactor.CyclomaticComplexity
category: Refactor
base_priority: 8
default_severity: Medium
options: []
---

McCabe cyclomatic complexity per function — independent paths through the body. Starts at `1` and adds `1` for each branching node: `if`, each non-default `case`, each loop, ternary, `catch`, and each `&&` / `||` / `??` in conditions. Plain `else` does not add a path. Default limit is `10`.

Cyclomatic and cognitive complexity often flag the same functions but rank them differently. Both are useful — cyclomatic captures "how many test cases do I need", cognitive captures "how hard is this to read". Run them together; the worst offenders fail both.

```ts
// flagged: 11 independent paths via &&/case/if combinations
function dispatch(event: Event) {
  switch (event.kind) {
    case "create": return event.payload && event.payload.id ? handleCreate(event) : null;
    case "update": return event.payload && event.payload.id ? handleUpdate(event) : null;
    case "delete": return event.payload && event.payload.id ? handleDelete(event) : null;
    case "ping": return null;
    default: return null;
  }
}
```

```ts
// fix: dispatch table flattens the case explosion
const handlers = { create: handleCreate, update: handleUpdate, delete: handleDelete } as const;
function dispatch(event: Event) {
  const handler = handlers[event.kind as keyof typeof handlers];
  if (!handler || !event.payload?.id) return null;
  return handler(event);
}
```
