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
