// Fixture for Design.EffectLeakage.
//
// @pure
//
// Flag — this file directly imports a known side-effecting module.
import * as fs from "fs";

export function loadConfig(): string {
  return fs.readFileSync("config.json", "utf8");
}
