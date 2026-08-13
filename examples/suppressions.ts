// Example fixture demonstrating cofferdam suppression directives.

// ============================================================
// Next-line directive: suppress all checks
// ============================================================

// cofferdam-disable-next-line
export function a(items: number[]) { return items.length; }
// This would normally trigger Design.ReadonlyArrayParam, but it's suppressed.

// ============================================================
// Next-line directive: suppress specific checks
// ============================================================

// cofferdam-disable-next-line Design.ReadonlyArrayParam
export function x(items: number[]) { return items.length; }
// Only ReadonlyArrayParam is suppressed here; other checks still fire.

// ============================================================
// Block directive: suppress all checks
// ============================================================

/* cofferdam-disable */
export function c(items: number[]) { return items.length; }
// Everything suppressed inside this block.
/* cofferdam-enable */

// This one is NOT suppressed:
export function p(items: number[]) { return items.length; }
// This should trigger Design.ReadonlyArrayParam normally.

// ============================================================
// Block directive: suppress specific checks
// ============================================================

/* cofferdam-disable Design.ReadonlyArrayParam */
export function m(items: number[]) { return items.length; }
// Only ReadonlyArrayParam suppressed; other findings still appear.
/* cofferdam-enable */

// ============================================================
// Next-line skips blank lines
// ============================================================

// cofferdam-disable-next-line

export function e(items: number[]) { return items.length; }
// The directive above applies to this line (first non-blank after directive).

// ============================================================
// Block with multiple check IDs
// ============================================================

/* cofferdam-disable Design.ReadonlyArrayParam, Design.MaxParameters */
export function g(items: number[]) { return items.length; }
// Both ReadonlyArrayParam and MaxParameters are suppressed on this line.
/* cofferdam-enable */

// ============================================================
// Real code: checking other violations
// ============================================================

function testFunction(
  a: number,
  b: number,
  c: number,
  d: number,
  e: number,
  f: number
) {
  // This function has more than 5 parameters (Design.MaxParameters would flag it).
  if (a == 0) {
    return b;
  }
  return c + d + e + f;
}
