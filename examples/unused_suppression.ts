// Fixture for Consistency.UnusedSuppression.
// Lines marked FLAG should produce a Consistency.UnusedSuppression finding.
// Lines marked OK should not.

// FLAG — no == / != on the next line, so the TripleEquals suppression is stale.
// cofferdam-ignore: Warning.TripleEquals: removed the old comparison
const x = 1 + 1;

// OK — there IS a == on the next line, so the suppression is still active.
// cofferdam-ignore: Warning.TripleEquals: intentional loose null check
if (x == null) {
  console.log("null");
}

// FLAG — range covers only safe code, no eval() call inside.
// cofferdam-ignore-start: Warning.NoEval
function safeFunction() {
  return 42;
}
// cofferdam-ignore-end

// OK — range covers a real eval() call.
// cofferdam-ignore-start: Warning.NoEval
function legacyFunction(code: string) {
  return eval(code); // eslint-disable-line
}
// cofferdam-ignore-end

// FLAG — whole-file suppression for TripleEquals but file has no == / !=
// (after this point). In practice the file-wide directive at line 1 would
// cover the entire file; here we just show the directive form.
// cofferdam-ignore-file: Warning.NoDebugger

// OK — broad form is Consistency.BroadSuppression's territory, not ours.
// cofferdam-ignore
const y = 2;
