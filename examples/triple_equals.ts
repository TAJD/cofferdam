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

// cd-w9i: null-operand cases — ESLint "eqeqeq: always-except-null" semantics.
// `x == null` / `x != null` are idiomatic JS/TS for "null OR undefined".
// Replacing with `=== null` changes behaviour, so all four shapes are exempt.
export function nullChecks(val: unknown, other: unknown) {
  if (val == null) return "null or undefined";   // OK — null-operand exemption
  if (val != null) return "defined";             // OK — null-operand exemption
  if (null == other) return "null or undefined"; // OK — null on LHS
  if (null != other) return "defined";           // OK — null on LHS
  return null;
}

// Parens around non-null side: null on RHS is still detected.
export function nullWithParens(a: boolean, b: boolean, val: unknown) {
  return (a || b) == null;                       // OK — null-operand exemption
}

// These SHOULD fire — null appears only as part of an identifier or string.
export function nonNullKeyword(nullable: unknown, y: unknown) {
  if (nullable == y) return true;                // flag: == (identifier, not keyword)
  if (y == "null") return true;                  // flag: == (string literal, not keyword)
}
