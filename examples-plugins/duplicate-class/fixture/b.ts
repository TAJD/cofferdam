// Fixture file B. Declares class `Widget`, which collides with the
// Widget declared in a.ts — DuplicateClassName should fire on the pair.

export class Widget {
  hello(): string {
    return "from b";
  }
}
