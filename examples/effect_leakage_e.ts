// Fixture for Design.EffectLeakage.
//
// Not flagged — no `@pure` tag, so no contract is being made even though
// this file directly imports a side-effecting module.
import * as fs from "fs";

export function loadOtherConfig(): string {
  return fs.readFileSync("other.json", "utf8");
}
