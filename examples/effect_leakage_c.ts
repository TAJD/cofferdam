// Fixture for Design.EffectLeakage — not itself @pure, just the
// side-effecting hop that effect_leakage_b.ts transitively depends on.
import * as fs from "fs";

export function readFromDisk(): string {
  return fs.readFileSync("data.txt", "utf8");
}
