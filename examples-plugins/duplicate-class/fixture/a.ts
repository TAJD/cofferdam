// Fixture file A. Declares class `Widget` and class `Unique`.
// `Widget` is also declared in b.ts — DuplicateClassName should fire.
// `Unique` is only declared here — should NOT fire.

export class Widget {
  hello(): string {
    return "from a";
  }
}

export class Unique {
  ping(): boolean {
    return true;
  }
}
