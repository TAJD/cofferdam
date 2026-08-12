// Example fixture demonstrating cofferdam suppression directives.

// ============================================================
// Next-line directive: suppress all checks
// ============================================================

// cofferdam-disable-next-line
let a = 1;
// This would normally trigger Refactor.PreferConstOverLet, but it's suppressed.

// ============================================================
// Next-line directive: suppress specific checks
// ============================================================

// cofferdam-disable-next-line Refactor.PreferConstOverLet
let x = 1;
// Only PreferConstOverLet is suppressed here; other checks still fire.

// ============================================================
// Block directive: suppress all checks
// ============================================================

/* cofferdam-disable */
let c = 1;
// Everything suppressed inside this block.
/* cofferdam-enable */

// This one is NOT suppressed:
let p = 1;
// This should trigger Refactor.PreferConstOverLet normally.

// ============================================================
// Block directive: suppress specific checks
// ============================================================

/* cofferdam-disable Refactor.PreferConstOverLet */
let m = 1;
// Only PreferConstOverLet suppressed; other findings still appear.
/* cofferdam-enable */

// ============================================================
// Next-line skips blank lines
// ============================================================

// cofferdam-disable-next-line

let e = 1;
// The directive above applies to this line (first non-blank after directive).

// ============================================================
// Block with multiple check IDs
// ============================================================

/* cofferdam-disable Refactor.PreferConstOverLet, Design.MaxParameters */
let g = 1;
// Both PreferConstOverLet and MaxParameters are suppressed on this line.
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
