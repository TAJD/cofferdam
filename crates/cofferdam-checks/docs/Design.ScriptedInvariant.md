# Design.ScriptedInvariant

Evaluates one or more `[invariants.scripted."rule-name"]` blocks in
`cofferdam.invariants.toml` using the v1 predicate DSL. Lets projects
encode architectural rules that don't fit the flat
`forbid_imports` / `require_imports` shape — file-level constraints,
cross-file `imports` / `exports` predicates, layer-conditional gates.

## Configuration

```toml
[invariants.scripted."controller-test-pair"]
when    = "file matches 'src/controllers/**/*.ts'"
require = "exists 'tests/' + basename(file)"
message = "Every controller needs a test under tests/"

[invariants.scripted."ui-no-localstorage"]
when    = "file matches 'ui/**'"
forbid  = "imports 'localStorage'"
message = "UI files must not touch localStorage directly"
```

Each rule has four fields:

* `when` (optional) — predicate that gates the rule. The rule only
  fires on files where `when` evaluates true. Omit to apply
  everywhere.
* `require` (optional) — predicate that must hold; the rule emits a
  finding when it evaluates false.
* `forbid` (optional) — predicate that must NOT hold; the rule emits
  a finding when it evaluates true.
* `message` — the literal text surfaced on each finding.

Exactly one of `require` / `forbid` must be set. Setting both, or
neither, is a config-load error.

## DSL vocabulary

See `docs/dsl-grammar.md` for the full grammar. v1 surface:

* Subjects: `file`, `file.path`, `file.layer`.
* Operators: `matches` (glob), `==` / `!=`, `in '<layer>'`, `imports`,
  `transitively imports`, `imports as type`, `imports as value`,
  `exports`.
* Functions: `basename(...)`, `dirname(...)`, `exists(...)`.
* Boolean glue: `and`, `or`, `not`, parentheses.
* String concat: `'a' + basename(file) + '.ts'`.

## Validation

DSL strings are parsed at config load (`cofferdam.invariants.toml`
load). Bad scripts fail fast with an actionable error pointing at the
offending rule + field, not at file 4000 of the run.

## Span semantics (v1)

Findings carry the file path with line/col 1:1. Per-edge spans for
`imports` predicates are reserved for a future MINOR bump.
