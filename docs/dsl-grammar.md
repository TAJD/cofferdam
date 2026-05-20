# `[invariants.scripted]` predicate DSL — v1 grammar

Authoritative grammar for the embedded scripting layer inside
`cofferdam.invariants.toml` (cd-9hp.1). The parser in
`cofferdam_core::dsl` implements this spec exactly; deviations are
bugs.

Inherits the schema-versioning policy: every spec carrying scripted
rules declares `schema_version = "1.0"` at the top of
`cofferdam.invariants.toml`. DSL grammar bumps follow the
`MAJOR.MINOR` rules in [docs/schema-versioning.md](./schema-versioning.md).

## TOML shape

```toml
[invariants.scripted."<rule-name>"]
when    = "<predicate>"
require = "<predicate>"
message = "<format-string>"
```

`<rule-name>` is the stable identifier used in suppression directives
and baseline entries. Conventions: lowercase, dash-separated, scoped
by domain (`controller-test-pair`, `domain-purity`,
`sql-no-nullable-fk`).

A rule fires when `when` evaluates true AND `require` evaluates false
on the same file. `message` is formatted with `{name}` substitutions
(see "Format strings" below) and emitted as the finding message.

Both `when` and `require` are optional. Defaults:

- `when` omitted ⇒ rule applies to every file in scope.
- `require` omitted ⇒ rule fires whenever `when` matches (used for
  pure denylist rules: "no file in `layer.app` may import
  `infra.db`" — the `forbid` operator in `when` is sufficient).

## Predicate grammar

```ebnf
predicate    = or_expr ;
or_expr      = and_expr ( "or" and_expr )* ;
and_expr     = not_expr ( "and" not_expr )* ;
not_expr     = [ "not" ] atom ;
atom         = "(" predicate ")"
             | call
             | comparison ;

comparison   = subject op operand ;
subject      = identifier | "(" predicate ")" ;  (* see Subject conventions *)
op           = "matches"          (* glob match — value is a glob string *)
             | "imports"          (* direct import — value is a module specifier *)
             | "transitively" "imports"   (* transitive-closure import *)
             | "imports" "as" "type"      (* type-only import *)
             | "imports" "as" "value"     (* value import *)
             | "exports"          (* file exports a named symbol *)
             | "in"               (* file is in a layer — value is layer name *)
             | "=="               (* string equality *)
             | "!="               (* string inequality *)
             ;
operand      = string | call | concat ;
concat       = operand "+" operand ;   (* left-associative *)
call         = identifier "(" [ args ] ")" ;
args         = predicate ( "," predicate )* ;

(* `forbid` and `require` are sugar that wraps an operator in negation
   for readability:
     forbid imports 'X'    ⇔   not imports 'X'
     require exports 'Y'   ⇔   exports 'Y'
   They are recognised only at the top of `when` / `require`. *)
top_predicate = ( "forbid" | "require" ) predicate | predicate ;

string       = "'" <chars> "'" | "\"" <chars> "\"" ;
identifier   = ident_start { ident_cont } ;
ident_start  = ASCII letter | "_" ;
ident_cont   = ASCII letter | ASCII digit | "_" | "." ;
```

## Subject conventions

`subject` is what the rule predicates against. v1 vocabulary:

| Subject | Domain | Meaning |
|---|---|---|
| `file` | TS, all | The file currently under evaluation |
| `file.path` | TS, all | The file's project-relative path string |
| `file.layer` | TS, all | The file's resolved layer name (or `null`) |
| `core.symbol(<name>)` | all | A declared symbol by name |
| `core.import(<spec>)` | all | An import edge by module specifier |
| `ts.declaration(<name>)` | TS-specific | A TypeScript declaration by name |

Namespaces are **reserved** for forward-compat with non-TS adapters
(cd-9hp.10). v1 implements only `core.*` and `ts.*`; an unregistered
namespace in a subject is a load-time error:

```
error: subject 'sql.column' uses unregistered namespace 'sql'
       (rule: 'controller-test-pair', cofferdam.invariants.toml:14)
       known namespaces: core, ts
       reopen cd-9hp.10 to ship a SQL adapter
```

## Operators

| Operator | Operand kind | v1 semantics | Forward-compat |
|---|---|---|---|
| `matches` | glob string | gitignore-style match against subject's path/identifier | — |
| `imports` | module specifier (string) | direct import edge exists from subject (a file) to that specifier | v2: routes to canonical graph |
| `transitively imports` | module specifier | transitive-closure of `imports` over the file graph | v1: implemented; v2: shifts to graph closure |
| `imports as type` / `imports as value` | module specifier | type-only vs value import (TS `import type`) | edge-typed traversal reserved for graph promotion |
| `exports` | symbol name (string) | subject file exports the named symbol | — |
| `in` | layer name | subject file resolves to the named layer | — |
| `==` / `!=` | string | string equality on path / layer / name | — |

`forbid X` is exactly `not X`; `require X` is exactly `X`. The wrappers
exist for the human reader — `forbid imports 'X'` reads more naturally
than `not imports 'X'` at the top of a predicate.

## Built-in functions

v1 ships three; new additions go through MINOR version bumps.

| Function | Signature | Meaning |
|---|---|---|
| `basename(p)` | `string → string` | Final path component, no extension |
| `dirname(p)` | `string → string` | Path without the final component |
| `exists(p)` | `string → bool` | A file exists at the project-relative path |

## Format strings

`message` is a TOML string where `{<expression>}` interpolates a
predicate result. The expression must yield a string; common forms:

- `{file}` — the current file's project-relative path
- `{file.layer}` — the current file's resolved layer (or empty)
- `{basename(file)}` — the file's basename

Curly braces in the literal message are escaped as `{{` and `}}`.

## Errors

The DSL is **fail-fast at config load**, not at file 4000. The parser
validates every rule when `cofferdam.invariants.toml` is first read.
Each error names the rule, the file location, and the canonical
syntax for the surface it tripped on.

| Class | Example |
|---|---|
| Syntax | `unexpected token ')' at column 24 (rule 'X', cofferdam.invariants.toml:18)` |
| Unknown subject | `unknown subject 'fyle' — did you mean 'file'?` |
| Unknown operator | `unknown operator 'imprts' — known: matches, imports, ...` |
| Unregistered namespace | `subject 'sql.column' uses unregistered namespace 'sql'; known: core, ts` |
| Bad string escape | `invalid escape \\q in string literal at column 12` |

A spec containing any DSL error is rejected wholesale; the engine
refuses to start until every rule parses.

## What the v1 grammar does NOT include

Reserved for v2 (graph-substrate promotion via cd-9hp.9) or later:

- **Quantifiers** (`exists`, `forall`) over corpus collections.
  Today's checks pattern-match against individual files; quantifiers
  need the graph substrate to be efficient.
- **Aggregation** (`count`, `min`, `max`).
- **Path-shape predicates** (`subject reaches target via edge.kind`).
- **Cross-rule references** (one rule depending on another's result).
- **User-defined functions** beyond the three built-ins.
- **Comments inside the predicate string** — keep predicates one-line.

If a real use case for any of these appears before cd-9hp.9 ships,
raise it as a separate v1.1 / v2 bead with a concrete example. The
grammar is designed to admit each addition through a MINOR bump
without breaking v1 rules.

## Two complete examples

```toml
schema_version = "1.0"

[invariants.scripted."controller-test-pair"]
when    = "file matches 'src/controllers/**/*.ts'"
require = "exists('tests/controllers/' + basename(file) + '.test.ts')"
message = "Every controller needs a registered test (expected 'tests/controllers/{basename(file)}.test.ts')"

[invariants.scripted."ui-no-localstorage"]
when    = "file matches 'src/components/ui/**'"
require = "forbid imports 'localStorage'"
message = "UI primitives must not touch `localStorage`; route storage through the persistence layer."
```

The first rule fires per-file when a controller exists but its
companion test file does not. The second fires when a UI primitive
imports `localStorage`. Both demonstrate the canonical TOML surface;
the parser turns each predicate string into a strongly-typed AST
that the runtime evaluator (`Design.ScriptedInvariant`) walks per
file.

## Implementation pointer

- Parser + AST: `crates/cofferdam-core/src/dsl/` (cd-9hp.1 ships
  this in three checkpoints — see the bead's `--design` field).
- Evaluator over flat corpus: same module, separate file.
- Engine integration: `Design.ScriptedInvariant` registered in
  `cofferdam-checks` like any other check, but its `run()` body
  walks the parsed AST rather than the source AST.
- Schema versioning: per [docs/schema-versioning.md](./schema-versioning.md).
