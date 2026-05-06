---
id: Warning.TripleEquals
category: Warning
base_priority: 15
default_severity: High
options: []
---

`==` and `!=` perform type coercion and are almost always a bug. Use `===` and `!==` instead. Walks every `BinaryExpression` and flags the equality operators.

**Null exemption** (`eqeqeq: always-except-null` semantics): `x == null`, `x != null`, `null == x`, and `null != x` are intentionally exempt. The `== null` pattern is idiomatic JavaScript/TypeScript shorthand for "value is `null` or `undefined`" — replacing it with `=== null` changes runtime behaviour. All four shapes produce no diagnostic.

```ts
// flagged
if (a == b) return true;
if (a != b) return false;
if (y == "null") return true;   // string literal, not the keyword
```

```ts
// fix
if (a === b) return true;
if (a !== b) return false;
```

```ts
// not flagged — null-operand exemption
if (val == null) return "null or undefined";
if (val != null) return "defined";
if (null == other) return "null or undefined";
if (null != other) return "defined";
```

```ts
// not relevant — relational operators don't coerce in the same way
if (a < b) return true;
if (a >= b) return false;
```

```toml
# The default (high) gates CI — explicit override only useful to demote.
[checks."Warning.TripleEquals"]
severity = "medium"
```
