// Fixture for Design.EffectLeakage (CD-138).
//
// Not flagged — the @pure tag documents sumItems specifically, and fs is
// only referenced inside the unrelated readConfigFile, so it isn't
// attributed to sumItems' own contract.
import * as fs from "fs";

export function readConfigFile(): string {
  return fs.readFileSync("config.json", "utf8");
}

// @pure
export function sumItems(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
