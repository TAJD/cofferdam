// Fixture for Consistency.UnusedSuppression.
// Lines marked FLAG should produce a Consistency.UnusedSuppression finding.
// Lines marked OK should not.

// FLAG — no `let` on the next line, so the PreferConstOverLet suppression is stale.
// cofferdam-ignore: Refactor.PreferConstOverLet: removed the old declaration
const x = 1 + 1;

// OK — there IS a `let` on the next line, so the suppression is still active.
// cofferdam-ignore: Refactor.PreferConstOverLet: intentional non-const declaration
let z = 5;

// FLAG — range covers only safe code, no `let` declaration inside.
// cofferdam-ignore-start: Refactor.PreferConstOverLet
function safeFunction() {
  return 42;
}
// cofferdam-ignore-end

// OK — range covers a real never-reassigned `let`.
// cofferdam-ignore-start: Refactor.PreferConstOverLet
function legacyFunction() {
  let value = 42;
  return value;
}
// cofferdam-ignore-end

// OK — whole-file suppression for PreferConstOverLet: the file still
// contains live `let` declarations above (`z`, `value`) that the check
// would flag, so this broader directive is considered to cover them too,
// even though narrower directives already suppress them locally.
// cofferdam-ignore-file: Refactor.PreferConstOverLet

// OK — broad form is Consistency.BroadSuppression's territory, not ours.
// cofferdam-ignore
const y = 2;
