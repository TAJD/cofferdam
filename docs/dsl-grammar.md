# `[invariants.scripted]` predicate DSL — v1 grammar

Authoritative grammar for the embedded scripting layer inside
`cofferdam.invariants.toml`. The parser in
`cofferdam_core::dsl` implements this spec exactly; deviations are
bugs.

Inherits the schema-versioning policy: every spec carrying scripted
rules declares `schema_version = "1.0"` at the top of
`cofferdam.invariants.toml`. DSL grammar bumps follow the
`MAJOR.MINOR` rules in [docs/schema-versioning.md](./schema-versioning.md).

## TOML shape

```toml
[invariants.scripted."<rule-name>"]
when    = "<predicate>"     # optional gate
require = "<predicate>"     # exactly one of require / forbid
forbid  = "<predicate>"     # exactly one of require / forbid
message = "<finding-text>"
```

`<rule-name>` is the stable identifier used in suppression directives
and baseline entries. Conventions: lowercase, dash-separated, scoped
by domain (`controller-test-pair`, `domain-purity`,
`sql-no-nullable-fk`).

A rule fires per file when:

- `when` (if set) evaluates true on the file, AND
- `require` (if set) evaluates false on the file, OR
- `forbid` (if set) evaluates true on the file.

Exactly one of `require` / `forbid` must be set. Setting both or
neither is a load-time error. `when` is optional; when omitted, the
rule applies to every file in scope. `message` is emitted as the
finding's message text.

Choosing `require` vs `forbid` is purely a readability call:
`require X` reads naturally for "X must hold"; `forbid Y` reads
naturally for "Y must not hold". They are not equivalent at the
schema level — pick the one that reads at the call site.

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

(* `top_predicate` is the parse entry point — a `predicate` optionally
   prefixed with `forbid` / `require`. v1 reads the TOML `require` /
   `forbid` fields as bare predicates (no prefix); the wrapper keywords
   live in the grammar so a future MINOR bump can collapse the two
   fields back into one without breaking existing rules. *)
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

Namespaces are **reserved** for forward-compatibility with non-TypeScript
adapters. v1 implements only `core.*` and `ts.*`; an unregistered
namespace in a subject is a load-time error:

```
subject 'sql.column' uses unregistered namespace 'sql'; known namespaces: core, ts
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

## Message text

v1 emits `message` verbatim — whatever literal TOML string the user
wrote lands in the finding. The
<code>&#123;&#123;file&#125;&#125;</code>-style interpolation surface
sketched in earlier design notes is reserved for v2 (cd-9hp.9). When
that ships, curly braces in literal messages will need escaping; v1
authors should avoid `{…}` substrings in messages today to keep
forward-compat free.

<!-- Curly-brace pairs are written in HTML-entity form because
     VitePress' Vue compiler scans rendered markdown for literal
     double-brace template interpolations even inside inline backtick
     code spans. Entity escapes keep the rendered output identical
     without tripping Vue. scripts/check-vitepress.mjs enforces this
     at pre-commit. -->


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

Reserved for v2 (graph-substrate promotion) or later:

- **Quantifiers** (`exists`, `forall`) over corpus collections.
  Today's checks pattern-match against individual files; quantifiers
  need the graph substrate to be efficient.
- **Aggregation** (`count`, `min`, `max`).
- **Path-shape predicates** (`subject reaches target via edge.kind`).
- **Cross-rule references** (one rule depending on another's result).
- **User-defined functions** beyond the three built-ins.
- **Comments inside the predicate string** — keep predicates one-line.

If you have a real use case for any of these, file it in Projektor with a
concrete example. The grammar is designed to admit each addition through a
MINOR bump without breaking v1 rules.

## Two complete examples

```toml
schema_version = "1.0"

[invariants.scripted."controller-test-pair"]
when    = "file matches 'src/controllers/**/*.ts'"
require = "exists('tests/' + basename(file))"
message = "Every controller needs a test file under tests/"

[invariants.scripted."ui-no-localstorage"]
when    = "file matches 'ui/**'"
forbid  = "imports 'localStorage'"
message = "UI files must not touch localStorage directly"
```

The first rule fires per file when a controller exists but its
sibling test file does not. The second fires when a UI file imports
`localStorage`. Both are pinned by the spec-contract fixtures
`scripted-file-level` and `scripted-cross-file` under
`crates/cofferdam-engine/tests/spec_contract/`; the parser turns
each predicate string into a strongly-typed AST that the runtime
evaluator (`Design.ScriptedInvariant`) walks per file.

## Implementation pointer

- Parser + AST: `crates/cofferdam-core/src/dsl/`.
- Evaluator over flat corpus: same module, separate file.
- Engine integration: `Design.ScriptedInvariant` registered in
  `cofferdam-checks` like any other check, but its `run()` body
  walks the parsed AST rather than the source AST.
- Schema versioning: per [docs/schema-versioning.md](./schema-versioning.md).
