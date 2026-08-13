// Fixture for Consistency.UnusedSuppression.
// Lines marked FLAG should produce a Consistency.UnusedSuppression finding.
// Lines marked OK should not.

// FLAG — the parameter is mutated, so ReadonlyArrayParam doesn't fire here;
// the suppression is stale.
// cofferdam-ignore: Design.ReadonlyArrayParam: legacy mutation guard
export function mutates(items: number[]) {
  items.push(1);
}

// OK — the parameter is never mutated, so ReadonlyArrayParam fires here and
// the suppression is live.
// cofferdam-ignore: Design.ReadonlyArrayParam: intentional mutable-array API
export function readsOnly(items: number[]) {
  return items.length;
}

// FLAG — range covers only safe code, no unmutated array param inside.
// cofferdam-ignore-start: Design.ReadonlyArrayParam
function safeFunction() {
  return 42;
}
// cofferdam-ignore-end

// OK — range covers a real never-mutated array param.
// cofferdam-ignore-start: Design.ReadonlyArrayParam
export function legacyFunction(items: number[]) {
  return items.length;
}
// cofferdam-ignore-end

// OK — whole-file suppression for ReadonlyArrayParam: the file still
// contains live `readsOnly`/`legacyFunction` findings above that the check
// would flag, so this broader directive is considered to cover them too,
// even though narrower directives already suppress them locally.
// cofferdam-ignore-file: Design.ReadonlyArrayParam

// OK — broad form is Consistency.BroadSuppression's territory, not ours.
// cofferdam-ignore
const y = 2;
