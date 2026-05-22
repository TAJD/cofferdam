// CI smoke fixture for the ts-morph type host (cd-9hp.2.4).
//
// Warning.UnusedNullCheck is type-aware: it only fires when cofferdam
// runs with the ts-morph type host (a project with tsconfig.json +
// ts-morph installed, and `[engine] type_aware` not disabled).
// scripts/check-type-host-smoke.mjs runs the built binary against this
// project from CI and asserts the flagged set below — proving the worker
// pool resolves real TypeScript types end-to-end, including across files.

import { type Widget, makeWidget } from "./widget";

// --- flagged: the operand's type already excludes the checked value ---

export function flaggedStrictNull(s: string): string {
  if (s !== null) return s; // redundant — string excludes null
  return "";
}

export function flaggedLoose(n: number): number {
  if (n != null) return n; // redundant — number excludes null and undefined
  return 0;
}

export function flaggedStrictUndefined(b: boolean): string {
  return b === undefined ? "x" : "y"; // redundant — boolean excludes undefined
}

// Cross-file: `w.id` resolves to `string` through the imported interface,
// so the guard is redundant. Fires only if project-wide resolution works.
export function flaggedCrossFile(w: Widget): string {
  if (w.id !== null) return w.id; // redundant — Widget.id is string
  return "";
}

// --- not flagged: the guard is meaningful -----------------------------

export function okNullable(s: string | null): string {
  if (s !== null) return s; // s really can be null
  return "";
}

export function okUndefined(s: string | undefined): string {
  if (s === undefined) return ""; // s really can be undefined
  return s;
}

// --- not flagged: any defeats the analysis ----------------------------

export function okAny(x: any): unknown {
  if (x != null) return x;
  return 0;
}

export const seed = makeWidget("seed");
