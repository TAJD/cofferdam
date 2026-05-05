// Fixture for Warning.TripleEquals.

export function loose(a: unknown, b: unknown) {
  if (a == b) {                  // flag: ==
    return true;
  }
  if (a != b) {                  // flag: !=
    return false;
  }
  return null;
}

export function strict(a: unknown, b: unknown) {
  if (a === b) return true;       // OK
  if (a !== b) return false;      // OK
  return null;
}

export function nested(x: unknown, y: unknown, z: unknown) {
  return x === y && z == "foo";   // flag: == in second clause
}

// cd-21d: null-operand cases — diagnostic fires but autofix is suppressed
// because `== null` matches both null and undefined, unlike `=== null`.
export function nullChecks(val: unknown, other: unknown) {
  if (val == null) return "null or undefined";   // flag: ==, NO autofix
  if (val != null) return "defined";             // flag: !=, NO autofix
  if (null == other) return "null or undefined"; // flag: ==, NO autofix (null on LHS)
  if (null != other) return "defined";           // flag: !=, NO autofix (null on LHS)
  return null;
}

// Parens around non-null side must not fool the null-detection.
export function nullWithParens(a: boolean, b: boolean, val: unknown) {
  return (a || b) == null;                       // flag: ==, NO autofix
}

// These SHOULD autofix — null appears only as part of an identifier or string.
export function nonNullKeyword(nullable: unknown, y: unknown) {
  if (nullable == y) return true;                // flag: ==, autofix OK (identifier)
  if (y == "null") return true;                  // flag: ==, autofix OK (string literal)
}
