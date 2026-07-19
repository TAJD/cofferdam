---
id: Refactor.PreferConstOverLet
category: Refactor
base_priority: -5
default_severity: Low
options: []
---

A `let` binding that's never reassigned anywhere in the file should be `const` — it signals to a reader that the value doesn't change, and rules out an entire class of reassignment bugs at compile time.

```ts
let total = price + tax; // flagged — never reassigned
return total;
```

```ts
// fix
const total = price + tax;
return total;
```

Reassignment is tracked by name across the whole file, including inside nested closures — a closure that captures and reassigns an outer `let` still counts, so the outer binding is correctly left alone:

```ts
let count = 0;
function increment() {
  count += 1; // reassignment, found even though it's in a nested function
}
```

Scoped to simple-identifier bindings only in this version — destructured bindings (`let { a, b } = obj`) are skipped. Tracking is name-based rather than true lexical scope: a shadowed `let` that shares a name with a reassigned variable in a different scope won't be flagged even if that specific binding is itself never reassigned. This is a deliberate false-negative, not a false-positive — the safer direction for a "should be const" suggestion.

Overlap with ESLint's `prefer-const` rule is expected and acceptable.

```toml
# cofferdam.toml
[checks."Refactor.PreferConstOverLet"]
severity = "medium"   # raise if your project treats this as more than a style nit
```
