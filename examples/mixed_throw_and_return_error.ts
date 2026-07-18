// Fixture for Refactor.MixedThrowAndReturnError.

// Flag — throws in one branch, returns an error-shaped object in a
// distinct branch: two competing idioms for the same class of failure.
export function parseConfig(input: string) {
  if (!input) {
    throw new Error("input required");
  }
  const parsed = tryParse(input);
  if (!parsed) {
    return { error: "invalid config" };
  }
  return parsed;
}

// Not flagged — only ever returns an error-shaped object, no throw.
export function loadResult() {
  return { error: null, value: 42 };
}

// Not flagged — throw and the only error-shaped return are in the
// exact same block (the return is unreachable dead code, not a
// competing idiom).
export function overlapping(x: number) {
  if (x < 0) {
    throw new Error("negative");
    return { error: "unreachable" };
  }
  return x * 2;
}

declare function tryParse(_input: string): { value: string } | null;
