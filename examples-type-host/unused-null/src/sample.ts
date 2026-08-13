// CI smoke fixture for the ts-morph type host (cd-9hp.2.4).
//
// Design.UnionExhaustivenessGap is type-aware: it only fires when
// cofferdam runs with the ts-morph type host (a project with
// tsconfig.json + ts-morph installed, and `[engine] type_aware` not
// disabled). scripts/check-type-host-smoke.mjs runs the built binary
// against this project from CI and asserts the flagged set below —
// proving the worker pool resolves real TypeScript types end-to-end,
// including across files.

import { type Status, type Widget, makeWidget } from "./widget";

type Direction = "up" | "down" | "left" | "right";

// --- flagged: a variant is unhandled and there's no default case ------

export function flaggedMissingOne(status: Status): string {
  switch (status) {
    case "active":
      return "on";
    case "inactive":
      return "off";
    // "pending" unhandled, no default
  }
  return "";
}

export function flaggedMissingTwo(dir: Direction): number {
  switch (dir) {
    case "up":
      return 0;
    case "down":
      return 1;
    // "left" and "right" unhandled, no default
  }
  return -1;
}

// Cross-file: `Status` is a literal union declared in ./widget and
// resolved here through `Widget.status`. Fires only if project-wide
// resolution works.
export function flaggedCrossFile(w: Widget): string {
  switch (w.status) {
    case "active":
      return "on";
    // "inactive" and "pending" unhandled, no default
  }
  return "";
}

// --- not flagged: every variant handled, or a default catches the rest -

export function okExhaustive(status: Status): string {
  switch (status) {
    case "active":
      return "on";
    case "inactive":
      return "off";
    case "pending":
      return "waiting";
  }
  return "";
}

export function okDefault(status: Status): string {
  switch (status) {
    case "active":
      return "on";
    default:
      return "off";
  }
}

export const seed = makeWidget("seed");
