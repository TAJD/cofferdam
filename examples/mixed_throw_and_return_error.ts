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

// Not flagged — the object return is a Result-shaped SUCCESS value
// (`error: null`), not a competing error idiom; the throw guards an
// invariant that's unrelated to the success/failure result shape.
export function divide(a: number, b: number) {
  if (b === 0) {
    throw new Error("division by zero is a programmer error");
  }
  return { error: null, value: a / b };
}

// Flag — brace-less guard clauses on both sides still count as
// distinct branches even without `{}`.
export function parseId(raw: string) {
  if (!raw) throw new Error("id required");
  if (raw.length > 64) return { error: "id too long" };
  return raw;
}

declare function tryParse(_input: string): { value: string } | null;
