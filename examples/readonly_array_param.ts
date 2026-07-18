// Fixture for Design.ReadonlyArrayParam.

// Flag — array param never mutated.
export function total(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}

// Flag — object-literal param never mutated.
export function describe(point: { x: number; y: number }): string {
  return `(${point.x}, ${point.y})`;
}

// Flag — Array<T> form, never mutated.
export function first(items: Array<string>): string | undefined {
  return items[0];
}

// Not flagged — already readonly.
export function sum(items: readonly number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}

// Not flagged — genuinely mutated via an array method.
export function sortInPlace(items: number[]): void {
  items.sort();
}

// Not flagged — mutated via index assignment.
export function zeroOut(items: number[]): void {
  items[0] = 0;
}

// Not flagged — mutated via field write.
export function reset(point: { x: number; y: number }): void {
  point.x = 0;
}

// Not flagged — passed through to another call; can't rule out mutation
// happening there, so this check conservatively skips it.
export function process(items: number[]): void {
  mutateSomewhere(items);
}

// Not flagged — passed to a constructor; same reasoning as a call.
export function wrap(items: number[]): void {
  new Wrapper(items);
}

declare function mutateSomewhere(items: number[]): void;
declare class Wrapper {
  constructor(items: number[]);
}
