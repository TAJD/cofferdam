// Fixture for Refactor.UnusedVariable.

// ─── Local let/const/var ───────────────────────────────────────────────────

export function locals() {
  const used = 1;
  const unused = 2;          // flag
  let alsoUnused = 3;        // flag
  var oldStyle = 4;          // flag
  return used;
}

// ─── Underscore prefix opts out ────────────────────────────────────────────

export function underscored() {
  const _ignored = 1;        // OK (underscore prefix)
  const _x = 2;              // OK
  const used = 3;
  return used;
}

// ─── Destructuring: each binding is independent ────────────────────────────

interface Pair {
  a: string;
  b: number;
}

export function destructure(obj: Pair) {
  const { a, b } = obj;      // `a` flagged; `b` is used
  return b;
}

export function destructureBoth(obj: Pair) {
  const { a, b } = obj;      // both used → no flags
  return a + b;
}

// ─── Rest patterns are intentional discards ────────────────────────────────

export function arrayRest(arr: number[]) {
  const [first, ...rest] = arr; // `first` is used; `rest` skipped (rest pattern)
  return first;
}

export function arrayRestUnused(arr: number[]) {
  const [, ...rest] = arr;      // `rest` skipped even when unused
  return rest.length === 0;     // (uses rest, just here for clarity)
}

export function paramRest(...args: unknown[]) {
  // ...args is a rest param; conventionally always considered used.
  // Even if we never read `args`, this should NOT flag.
  void args;
  return 0;
}

// ─── Function parameters ───────────────────────────────────────────────────

export function paramFlag(used: number, unused: string) {
  // `unused` flagged. `used` returned below.
  return used;
}

export function paramOk(used: number, _ignored: string) {
  // `_ignored` opts out via underscore prefix.
  return used;
}

// ─── Catch parameter ───────────────────────────────────────────────────────

export function caughtAndIgnored() {
  try {
    return JSON.parse("");
  } catch (e) {                // flag: `e` declared but never read
    return null;
  }
}

export function caughtAndUsed() {
  try {
    return JSON.parse("");
  } catch (e) {
    return String(e);          // OK
  }
}

export function caughtUnderscored() {
  try {
    return JSON.parse("");
  } catch (_) {                // OK (underscore prefix)
    return null;
  }
}

// ─── Nested scopes — outer use of inner-binding name doesn't count ─────────

export function nestedScope() {
  function inner() {
    const value = 1;           // flag (only used in `inner` would matter; never read)
    return 0;
  }
  const value = 99;            // outer `value` used below — distinct symbol
  inner();
  return value;
}

// ─── Module-scope is intentionally NOT flagged in v1 ───────────────────────

const TOP_LEVEL_UNUSED = 42;   // OK in v1 (module scope skipped — see file header)
void TOP_LEVEL_UNUSED;         // touch it so even a future stricter rule wouldn't flag here

// ─── TS parameter properties (cd-sh72 / gh #44) ────────────────────────────
//
// `constructor(private ctx: T)` is both a parameter AND a class field. A
// read through `this.<name>` counts as use, even though oxc doesn't resolve
// the member access back to the parameter symbol.

export class ParamPropUsedViaThis {
  constructor(private ctx: { value: number }) {} // OK — read via this.ctx below
  getValue(): number {
    return this.ctx.value;
  }
}

export class ParamPropUsedViaBracket {
  constructor(private store: { count: number }) {} // OK — read via this['store']
  count(): number {
    return this["store"].count;
  }
}

export class ParamPropNeverRead {
  constructor(private orphan: number) {} // flag — declared, never read via this
}

export class PlainUnusedCtorParam {
  constructor(plain: number) {} // flag — no modifier, so a normal positional param
}

// Per-class scoping: same field name `dup` in two classes. The first reads
// it via this (OK); the second never does (flag). Proves the this-read set
// is scoped to the enclosing class, not the whole file.

export class ReadsDup {
  constructor(private dup: number) {}
  read(): number {
    return this.dup;
  }
}

export class NeverReadsDup {
  constructor(private dup: number) {} // flag
}
