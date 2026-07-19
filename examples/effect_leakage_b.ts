// Fixture for Design.EffectLeakage.
//
// @pure
//
// Flag — the side effect is one hop away, in effect_leakage_c.ts.
import { readFromDisk } from "./effect_leakage_c";

export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0) + readFromDisk().length;
}
