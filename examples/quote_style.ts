// Fixture for Consistency.QuoteStyle
//
// This file has 6 single-quoted strings and 2 double-quoted strings.
// Dominant style = single quotes (>50%).
// Lines marked [FLAG] should produce a Consistency.QuoteStyle finding.
// Lines marked [OK]   should NOT be flagged.

// OK: single-quoted strings (dominant style)
const a = 'alpha';        // [OK]
const b = 'beta';         // [OK]
const c = 'gamma';        // [OK]
const d = 'delta';        // [OK]
const e = 'epsilon';      // [OK]
const f = 'zeta';         // [OK]

// FLAG: double-quoted strings (minority style)
const g = "eta";          // [FLAG]
const h = "theta";        // [FLAG]

// OK: forced to use double quotes because content contains a single quote
const apostrophe = "don't";  // [OK] — switching would require escaping

// OK: JSX attribute strings are excluded from quote-style accounting
// (uncomment the JSX line if you have TSX support in your check runner)
// const el = <Foo className="jsx-attr" />;

// OK: template literals are not string literals — never counted
const tmpl = `template`;  // [OK]
