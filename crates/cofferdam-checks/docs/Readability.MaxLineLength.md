---
id: Readability.MaxLineLength
category: Readability
base_priority: -5
default_severity: Low
options: [limit]
---

Lines longer than the configured limit are harder to scan and review. The check is a pure text-line scan — no AST traversal, so it runs even when a file fails to parse.

```ts
// flagged: > 120 columns
const config = { name: "very long literal", flags: ["a", "b", "c", "d", "e", "f"], description: "..." };
```

```ts
// fix: break across lines
const config = {
  name: "very long literal",
  flags: ["a", "b", "c", "d", "e", "f"],
  description: "...",
};
```

```toml
[checks."Readability.MaxLineLength"]
limit = 100
severity = "info"
```
