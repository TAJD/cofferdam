// Fixture for Design.EffectLeakage.
//
// @pure
//
// Not flagged — genuinely pure, no side-effecting module anywhere in
// its import chain.
export function computeAverage(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0) / items.length;
}
