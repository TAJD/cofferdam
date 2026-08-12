// Fixture for Consistency.BroadSuppression (cd-81a.4 broad-form info).

// FLAG — broad form silences every check on next line.
// cofferdam-ignore
const a = 1;

// OK — scoped form is auditable.
// cofferdam-ignore: Refactor.PreferConstOverLet: legacy declaration preserved for parity
let one = 1;

// OK — scoped form with only the id.
// cofferdam-ignore: Refactor.PreferConstOverLet
let two = 1;

// OK — block / file variants are out of scope for this check.
// cofferdam-ignore-start: Refactor.PreferConstOverLet
let three = 1;
// cofferdam-ignore-end

// OK — ESLint-style alias is out of scope (handled by suppression engine,
// but not flagged by BroadSuppression — bead scopes the diagnostic to the
// Biome `cofferdam-ignore` form only).
// cofferdam-disable-next-line
let four = 1;

// FLAG — second occurrence; the check fires on every directive line.
// cofferdam-ignore
const b = 2;
