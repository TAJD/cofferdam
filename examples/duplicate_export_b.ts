// Pair with duplicate_export_a.ts. Three of these collide:
//   - parseDate (function in A, function here)
//   - helper    (const in A, function here — same name still collides)
//   - Foo       (class in A, class here)
// `unique_to_b` should NOT be flagged.

export function parseDate(s: string): Date {
  return new Date(Date.parse(s));
}

export function helper(x: number): number {
  return x + 1;
}

export const unique_to_b = 42;

export class Foo {
  greet() {
    return "hi from B";
  }
}
