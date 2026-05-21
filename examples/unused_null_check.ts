// Fixture for Warning.UnusedNullCheck (cd-9hp.2.3).
//
// This check is TYPE-AWARE: it only fires when cofferdam runs with the
// ts-morph type host (a project with tsconfig.json + ts-morph installed
// and `[engine] type_aware` enabled). Running `cofferdam check` on this
// file inside the cofferdam repo (a Rust project, no type host) produces
// NO findings — that's expected. The decision logic is pinned by unit
// tests with a stub oracle in `cofferdam-checks/src/warning.rs`, and the
// end-to-end path by a gated test against a real ts-morph project.

// --- flagged: type already excludes the checked value -----------------

export function flaggedStrictNull(s: string) {
  if (s !== null) return s; // redundant — string excludes null
  return "";
}

export function flaggedLoose(n: number) {
  if (n != null) return n; // redundant — number excludes null AND undefined
  return 0;
}

export function flaggedStrictUndefined(b: boolean) {
  return b === undefined ? "x" : "y"; // redundant — boolean excludes undefined
}

// --- not flagged: the guard is meaningful -----------------------------

export function okNullable(s: string | null) {
  if (s !== null) return s; // s really can be null
  return "";
}

export function okUndefined(s: string | undefined) {
  if (s === undefined) return ""; // s really can be undefined
  return s;
}

// --- not flagged: any/unknown defeat the analysis ---------------------

export function okAny(x: any) {
  if (x != null) return x;
  return 0;
}

// --- not relevant: not a nullish comparison ---------------------------

export function notNullish(a: number, b: number) {
  return a !== b; // neither operand is null/undefined
}
