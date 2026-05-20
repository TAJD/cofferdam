// fixture.ts — input to `cofferdam check` for the BrandCasing plugin (cd-7e4).
//
// Comments label the expected outcome on each line. The plugin should emit
// exactly 2 issues, on the lines marked FLAG #1 and FLAG #2. Every other
// occurrence of the trigger word is exempted by one of the rules in the
// design doc (docs/plugin-sdk-e2e.md §1).

import { Examplco } from "./brand";        // OK: identifier import
import type { ExamplcoClient } from "./b"; // OK: type-only identifier

class ExamplcoSdk {                         // OK: identifier (class name)
  greet(): string {
    return "Welcome to Examplco!";          // FLAG #1: string literal
  }
}

export const HEADER = `Examplco — go faster`; // FLAG #2: template literal

// Plugin-level escape hatch — `// brand:ignore` exempts the next line.
// brand:ignore — legacy fixture asserting on the old casing
export const LEGACY = "Examplco (legacy)";  // EXEMPT (plugin magic comment)

// Engine-level suppression from cd-81a.4 — different mechanism, same effect.
// cofferdam-ignore: BrandCasing: see ROVI-481 — copywriter approved exception
export const CAMPAIGN = "Examplco Spring Sale"; // EXEMPT (engine suppression)

// Comment with the trigger word: Examplco is fine in dev context. // EXEMPT
/** JSDoc mentioning Examplco in passing. */                       // EXEMPT
/* Block comment: Examplco here is also fine. */                   // EXEMPT

export function ok(): string {
  return "EXAMPLCO all caps — fine.";       // OK: brand spelled correctly
}

// Identifier-only line — even though `Examplco` appears, it's a member
// access on an imported namespace, not display copy.
export const VERSION = Examplco.version;    // OK

// Allowlist override (from cofferdam.toml [checks."BrandCasing"]):
// allowedAliases = ["ExamplcoClient", "ExamplcoCdn"] — these names are
// permitted even when they appear inside string literals.
export const HINT: ExamplcoClient = "ExamplcoClient" as unknown as ExamplcoClient; // EXEMPT (allowlist)
