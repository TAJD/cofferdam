// Fixture for Design.MaxParameters (limit 5).

// OK — 3 params
export function ok(a: number, b: number, c: number) { return a + b + c; }

// Flag — 6 params
export function tooMany(a: number, b: number, c: number, d: number, e: number, f: number) {
  return a + b + c + d + e + f;
}

// OK — destructured object counts as 1 param even with many keys
export function destructuredOk({ a, b, c, d, e, f, g }: { a: number; b: number; c: number; d: number; e: number; f: number; g: number }) {
  return a + b + c + d + e + f + g;
}

// Flag — 6 params, mixing rest, defaults, destructure
export function mixedTooMany(a: number, b = 1, c = 2, { d, e }: { d: number; e: number }, ...rest: number[]) {
  return a + b + c + d + e + rest.length;
}

// Arrow function — 6 params, flag
export const arrowTooMany = (a: number, b: number, c: number, d: number, e: number, f: number) => a + b + c + d + e + f;
