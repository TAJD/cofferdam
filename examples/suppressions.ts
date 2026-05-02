// Example fixture demonstrating cofferdam suppression directives.

// ============================================================
// Next-line directive: suppress all checks
// ============================================================

// cofferdam-disable-next-line
if (a == b) {
  // This would normally trigger Warning.TripleEquals, but it's suppressed.
}

// ============================================================
// Next-line directive: suppress specific checks
// ============================================================

// cofferdam-disable-next-line Warning.TripleEquals
if (x == y) {
  // Only TripleEquals is suppressed here; other checks still fire.
}

// ============================================================
// Block directive: suppress all checks
// ============================================================

/* cofferdam-disable */
if (c == d) {
  // Everything suppressed inside this block.
}
/* cofferdam-enable */

// This one is NOT suppressed:
if (p == q) {
  // This should trigger Warning.TripleEquals normally.
}

// ============================================================
// Block directive: suppress specific checks
// ============================================================

/* cofferdam-disable Warning.TripleEquals */
if (m == n) {
  // Only TripleEquals suppressed; other findings still appear.
}
/* cofferdam-enable */

// ============================================================
// Next-line skips blank lines
// ============================================================

// cofferdam-disable-next-line

if (e == f) {
  // The directive above applies to this line (first non-blank after directive).
}

// ============================================================
// Block with multiple check IDs
// ============================================================

/* cofferdam-disable Warning.TripleEquals, Design.MaxParameters */
if (g == h) {
  // Both TripleEquals and MaxParameters are suppressed on this line.
}
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
