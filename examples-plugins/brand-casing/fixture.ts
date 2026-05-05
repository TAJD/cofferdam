// fixture.ts — input to `cofferdam check` for the BrandCasing plugin (cd-7e4).
//
// Comments label the expected outcome on each line. The plugin should emit
// exactly 2 issues, on the lines marked FLAG #1 and FLAG #2. Every other
// occurrence of the trigger word is exempted by one of the rules in the
// design doc (docs/plugin-sdk-e2e.md §1).

import { Rovikore } from "./brand";        // OK: identifier import
import type { RovikoreClient } from "./b"; // OK: type-only identifier

class RovikoreSdk {                         // OK: identifier (class name)
  greet(): string {
    return "Welcome to Rovikore!";          // FLAG #1: string literal
  }
}

export const HEADER = `Rovikore — go faster`; // FLAG #2: template literal

// Plugin-level escape hatch — `// brand:ignore` exempts the next line.
// brand:ignore — legacy fixture asserting on the old casing
export const LEGACY = "Rovikore (legacy)";  // EXEMPT (plugin magic comment)

// Engine-level suppression from cd-81a.4 — different mechanism, same effect.
// cofferdam-ignore: BrandCasing: see ROVI-481 — copywriter approved exception
export const CAMPAIGN = "Rovikore Spring Sale"; // EXEMPT (engine suppression)

// Comment with the trigger word: Rovikore is fine in dev context. // EXEMPT
/** JSDoc mentioning Rovikore in passing. */                       // EXEMPT
/* Block comment: Rovikore here is also fine. */                   // EXEMPT

export function ok(): string {
  return "ROVIKORE all caps — fine.";       // OK: brand spelled correctly
}

// Identifier-only line — even though `Rovikore` appears, it's a member
// access on an imported namespace, not display copy.
export const VERSION = Rovikore.version;    // OK

// Allowlist override (from cofferdam.toml [checks."BrandCasing"]):
// allowedAliases = ["RovikoreClient", "RovikoreCdn"] — these names are
// permitted even when they appear inside string literals.
export const HINT: RovikoreClient = "RovikoreClient" as unknown as RovikoreClient; // EXEMPT (allowlist)
