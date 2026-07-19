// Fixture for Design.EffectLeakage (CD-138).
//
// Flag — the @pure-tagged function itself directly references fs,
// unlike effect_leakage_f.ts where fs is only used by an unrelated
// function.
import * as fs from "fs";

export function unusedHelper(): number {
  return 1;
}

// @pure
export function readSettingsFile(): string {
  return fs.readFileSync("settings.json", "utf8");
}
