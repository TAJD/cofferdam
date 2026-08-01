// Fixture for Refactor.PreferConstOverLet.

// Flag — never reassigned.
export function total(price: number, tax: number) {
  let total = price + tax;
  return total;
}

// Not flagged — reassigned directly.
export function withDiscount(price: number, rate: number) {
  let price2 = price;
  price2 = price2 - price2 * rate;
  return price2;
}

// Not flagged — reassigned via increment.
export function countUp(limit: number) {
  let count = 0;
  while (count < limit) {
    count++;
  }
  return count;
}

// Not flagged — reassigned inside a nested closure that captures it.
export function makeCounter() {
  let count = 0;
  return function increment() {
    count += 1;
    return count;
  };
}

// Not flagged — const is already correct, nothing to do.
export function alreadyConst(a: number, b: number) {
  const sum = a + b;
  return sum;
}

// Not flagged (MVP scope) — destructured bindings are skipped.
export function destructured(obj: { a: number; b: number }) {
  let { a, b } = obj;
  return a + b;
}

// Not flagged — reassigned via object destructuring-assignment.
export async function reassignedViaObjectDestructure(
  fetchToken: () => Promise<{ token: string; slug: string }>,
) {
  let token: string;
  let slug: string;
  ({ token, slug } = await fetchToken());
  return token + slug;
}

// Not flagged — reassigned via array destructuring-assignment.
export function reassignedViaArrayDestructure(pair: () => [number, number]) {
  let first: number;
  let second: number;
  [first, second] = pair();
  return first + second;
}

// Not flagged — a `for...of` loop head reassigns its destructured target
// on every iteration, not via an `AssignmentExpression`.
export function reassignedViaForOfDestructure(pairs: [number, number][]) {
  let l: number;
  let r: number;
  let sum = 0;
  for ([l, r] of pairs) {
    sum += l + r;
  }
  return sum;
}
