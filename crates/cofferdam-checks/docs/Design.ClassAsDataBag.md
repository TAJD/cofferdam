---
id: Design.ClassAsDataBag
category: Design
base_priority: -5
default_severity: Low
options: []
---

A class with no behaviour beyond storing fields — no methods other than a constructor that only assigns fields, no inheritance, no `implements` clause, no decorators — is a candidate for a plain type/interface instead of a class.

```ts
class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}
```

```ts
// fix
type Point = { x: number; y: number };
```

Escape hatches — none of these are flagged, even though they might otherwise look like a data bag:

```ts
// Parameter properties: the standard DI pattern without decorators.
class Point {
  constructor(private readonly x: number, private readonly y: number) {}
}

// implements clause: a structural contract, not just storage.
class Point implements Shape {
  constructor(public x: number, public y: number) {}
}

// Inheritance (including built-ins like Error): legitimate non-FP use.
class NotFoundError extends Error {
  constructor(public id: string) {
    super(`not found: ${id}`);
  }
}

// A decorator.
@Injectable()
class Config {
  constructor(public apiUrl: string) {}
}

// Constructor does more than assign fields — that's behavior.
class Point {
  x: number;
  constructor(x: number) {
    if (x < 0) throw new Error("x must be non-negative");
    this.x = x;
  }
}
```

Scope note: a class with no constructor at all is only flagged if it declares at least one plain field — a genuinely empty class (`class Marker {}`) is a nominal-type marker pattern, a different concern this check doesn't address.

```toml
# cofferdam.toml
[checks."Design.ClassAsDataBag"]
severity = "medium"   # raise if your project leans heavily on plain types over classes
```
