---
id: Design.ReadonlyArrayParam
category: Design
base_priority: 5
default_severity: Low
options: []
---

A function parameter declared with a mutable array (`T[]`, `Array<T>`) or object-literal (`{ x: number }`) type, where nothing in the function body mutates it — a missed opportunity for a `readonly` guarantee the type checker would enforce for free.

```ts
function total(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

```ts
// fix — readonly is a compiler-enforced promise, not just a convention
function total(items: readonly number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
```

Not flagged — the parameter is genuinely mutated in the body:

```ts
function sortInPlace(items: number[]): void {
  items.sort();
}
```

Not flagged — the parameter is passed through to another call, which might mutate it; this check can't see across the call graph, so it conservatively skips rather than risk a false positive:

```ts
function process(items: number[]): void {
  mutateSomewhere(items);
}
```

Scope: declared-type syntax only — the parameter's own `TSTypeAnnotation` is read directly from the AST, not resolved through the ts-morph type host, so an untyped or inferred parameter isn't checked. A type already wrapped in `readonly`/`ReadonlyArray<T>`/`Readonly<T>` is never a candidate. Only single-level assignment/index writes and array-mutating method calls (`.push`, `.splice`, ...) on the parameter's own root identifier are recognised as "mutated" — a nested function or arrow declared inside the callback is a separate scope and isn't descended into, mirroring `Refactor.MutatedParameter`. A mutation inside a nested closure (`items.forEach(() => items.push(1))`) is therefore not seen, and a param handed off wrapped in another expression (`f({ items })`, a ternary arm) isn't recognised as an escape — only a bare identifier/spread passed directly as an argument is.
