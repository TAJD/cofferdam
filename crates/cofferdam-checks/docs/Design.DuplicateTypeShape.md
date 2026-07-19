---
id: Design.DuplicateTypeShape
category: Design
base_priority: 5
default_severity: Medium
options: []
---

Two files independently declare interfaces (or object-literal type aliases) with near-identical field shapes under different names — likely should be a single shared type.

```ts
// user.ts
interface User {
  id: string;
  name: string;
  email: string;
}
```

```ts
// customer.ts
interface Customer {
  id: string;
  name: string;
  email: string;
}
```

```ts
// fix — one shared type, imported where needed
interface Contact {
  id: string;
  name: string;
  email: string;
}
```

Not flagged — too few fields to distinguish real duplication from coincidence:

```ts
interface Point {
  x: number;
  y: number;
}

interface Size {
  x: number;
  y: number;
}
```

Not flagged — shapes diverge enough to be different types:

```ts
interface Draft {
  id: string;
  title: string;
  body: string;
}

interface Published {
  id: string;
  title: string;
  publishedAt: string;
}
```

Matching is structural and textual: two types are compared field-by-field on name and the type annotation's raw source text, scored as the fraction of the union of both types' field names that match on both. A pair scoring 80% or higher is flagged; below 3 fields, a shape is never compared at all (too easy to coincidentally overlap) — unless it has a non-empty `extends` clause (see below). Only `interface` declarations and `type X = { ... }` object-literal aliases are considered — mapped types, unions, and other type-alias forms are not. Only plain property signatures with an identifier key count — method signatures, index signatures, call/construct signatures, and computed/private keys are all ignored, so two interfaces differing only in their methods can still score a match, and a signature-heavy, property-light interface can silently drop below the 3-field floor. The `optional`/`readonly` modifiers aren't part of the comparison either: `id?: string` and `id: string` are treated as the same field. The type-text comparison is textual, not semantic: `string` and an aliased `type Str = string` used as a field's annotation would not be considered equal, and whitespace differences (`string | null` vs `string|null`) also compare unequal.

`extends` clauses: an interface's inherited fields aren't visible in its body, so it's only ever compared against another interface sharing the exact same (sorted) set of heritage names — comparing declared bodies across different bases would risk false positives/negatives against the interface's real (inherited) shape. When both sides share a base, that shared base is itself a duplication signal even if neither side declares any fields of its own, so an `extends`-clause interface bypasses the 3-field floor entirely. Heritage names are compared by the base identifier's text alone, not its generic type arguments — `extends Base<string>` and `extends Base<number>` are treated as the same base.

Clustering: matches are transitively grouped, not just pairwise — three (or more) mutually-similar types produce one finding naming every member of the group, rather than one finding per pair.
