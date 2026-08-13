---
id: Refactor.SideEffectInMapCallback
category: Refactor
base_priority: -5
default_severity: Low
options: []
---

A callback passed to `.map`/`.filter` that mutates something outside itself (an outer-scope variable) or calls a known side-effecting function (`console.*`), instead of purely computing and returning a value — a loop wearing a map costume.

```ts
const seen: number[] = [];
const doubled = items.map((item) => {
  seen.push(item); // mutates outer-scope `seen`
  return item * 2;
});
```

```ts
// fix — separate the accumulation from the transform
const doubled = items.map((item) => item * 2);
items.forEach((item) => seen.push(item));
```

Not flagged — the callback only mutates/declares locals of its own:

```ts
const doubled = items.map((item) => {
  const scaled = item * 2;
  return scaled;
});
```

Also flagged — writing into an outer-scope object property or array index, not just calling an array-mutating method:

```ts
const state = { count: 0 };
const withCount = items.map((item) => {
  state.count += 1; // writes into outer-scope `state`
  return item;
});
```

Not flagged — a `.map`/`.filter` callback whose return value happens to be discarded, with no other side effect, is a distinct "use `.forEach` instead" concern this check doesn't cover (v1 scope, per the ticket):

```ts
items.map((item) => process(item));
```

Scope: `.forEach` itself is never inspected — a side effect there is the whole point. Only the callback's own body is inspected; a nested function or arrow declared inside the callback is a separate scope and isn't descended into. `console.*` is currently the only side-effecting-call denylist entry. Member/index assignment targets (`x.y = ...`, `x[y] = ...`) are only checked at the root receiver — a destructured local (`const { acc } = ctx;`) isn't recognised as local, a known name-based limitation.
