// Fixture for Design.ClassAsDataBag.

// Flag — fields only, constructor just assigns them.
export class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}

// Flag — no constructor at all, just plain fields.
export class Bare {
  a: number = 0;
  b: string = "";
}

// Not flagged — parameter properties (standard DI without decorators).
export class ConfigService {
  constructor(
    private readonly apiUrl: string,
    private readonly timeoutMs: number,
  ) {}
}

// Not flagged — implements clause (structural contract).
export interface Shape {
  area(): number;
}
export class Square implements Shape {
  constructor(public side: number) {}
  area(): number {
    return this.side * this.side;
  }
}

// Not flagged — extends a built-in (Error subclassing).
export class NotFoundError extends Error {
  constructor(public id: string) {
    super(`not found: ${id}`);
  }
}

// Not flagged — has a decorator.
function Injectable() {
  return function (target: unknown) {
    return target;
  };
}
@Injectable()
export class Config {
  constructor(public apiUrl: string) {}
}

// Not flagged — compound assignment reads before writing (behavior),
// even though the target still looks like `this.<field>`.
export class Weird {
  x: number;
  constructor(x: number) {
    this.x = 0;
    this.x += x;
  }
}

// Not flagged — constructor does more than assign fields.
export class Validated {
  x: number;
  constructor(x: number) {
    if (x < 0) {
      throw new Error("x must be non-negative");
    }
    this.x = x;
  }
}

// Not flagged — has a real method beyond the constructor.
export class Counter {
  count: number;
  constructor() {
    this.count = 0;
  }
  increment(): void {
    this.count += 1;
  }
}
