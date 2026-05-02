// Should flag `tooMuch` (>10) but NOT `simple` or `mediumOk`.
// Default limit: 10.

// Cyclomatic 1 — no branches.
export function simple(x: number): number {
  return x + 1;
}

// Cyclomatic ~9: 1 + 4 ifs + 3 logical ops + 1 ternary.
export function mediumOk(a: number, b: number, c: number): string {
  if (a > 0) return "a";
  if (b > 0) return "b";
  if (c > 0 && a < 10) return "ab";
  if (a === b || c === 0) return "eq";
  return a > b ? "ab" : "ba";
}

// Cyclomatic ~13: enough branches to clearly exceed the limit.
export function tooMuch(x: number, y: number, z: number, mode: string): string {
  if (x > 0 && y > 0) {
    if (z > 0 || mode === "all") {
      for (let i = 0; i < x; i++) {
        if (i % 2 === 0) {
          while (i > 0 && i < y) {
            i++;
          }
        }
      }
    }
  }
  switch (mode) {
    case "a":
      return "A";
    case "b":
      return "B";
    case "c":
      return "C";
    default:
      return "?";
  }
}
