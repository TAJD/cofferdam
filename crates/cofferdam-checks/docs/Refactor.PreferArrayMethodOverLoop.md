---
id: Refactor.PreferArrayMethodOverLoop
category: Refactor
base_priority: -5
default_severity: Low
options: []
---

A `for`/`for-of` loop whose entire body builds one accumulator array by pushing a computed value each iteration is more clearly expressed with `.map()` (unconditional) or `.filter()` (gated by a single `if`).

```ts
const doubled: number[] = [];
for (const n of nums) {
  doubled.push(n * 2); // flagged — could be `.map()`
}
```

```ts
// fix
const doubled = nums.map((n) => n * 2);
```

```ts
const evens: number[] = [];
for (const n of nums) {
  if (n % 2 === 0) {
    evens.push(n); // flagged — could be `.filter()`
  }
}
```

```ts
// fix
const evens = nums.filter((n) => n % 2 === 0);
```

When the `if`'s consequent pushes a separately computed value rather than the raw iterated element, the suggestion is `.filter().map()` (a filter, then a transform) rather than a plain `.filter()`, since the loop is doing both:

```ts
const labels: string[] = [];
for (const n of nums) {
  if (n % 2 === 0) {
    const label = `even:${n}`; // flagged — `.filter().map()`, not just `.filter()`
    labels.push(label);
  }
}
```

Deliberately narrow to avoid false positives: the loop body (or the `if`'s consequent, for the filter shape) must match one of exactly two shapes and nothing else —

1. a single `arr.push(<expr>)` statement, or
2. a `const`/`let` declaration with an initializer, immediately followed by `arr.push(<thatName>)`.

Any other statement in the body — an early `break`/`continue`, a second accumulator, a side effect beyond the single push, an `if`/`else` — disqualifies the match entirely and the loop is left alone.

```toml
# cofferdam.toml
[checks."Refactor.PreferArrayMethodOverLoop"]
severity = "medium"   # raise if your project prefers array methods strongly
```
