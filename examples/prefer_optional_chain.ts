// Fixture for Refactor.PreferOptionalChain.

interface User {
  profile?: {
    name?: string;
    address?: {
      city?: string;
    };
  };
  greet?(): void;
}

export function shallow(u: User | null) {
  return u && u.profile;                     // flag: a && a.b
}

export function deep(u: User | null) {
  return u && u.profile && u.profile.name;   // flag: chain (both halves)
}

export function withCall(u: User | null) {
  return u && u.greet();                     // flag: call on member
}

export function computed(arr: number[] | null) {
  return arr && arr[0];                      // flag: computed member
}

// Different prefixes — NOT flagged.
export function unrelated(a: unknown, b: { c: string } | null) {
  return a && b.c;                           // OK (a !== b)
}

// LHS is a call — NOT flagged (side-effect repetition).
export function callLhs(get: () => User | null) {
  return get() && get().profile;             // OK (call !== call)
}

// Already an optional chain — NOT flagged (different AST shape).
export function alreadyOptional(u: User | null) {
  return u?.profile?.name;                   // OK
}

// Bare boolean conjunction — NOT flagged.
export function booleans(x: boolean, y: boolean) {
  return x && y;                             // OK (y is not a member of x)
}
