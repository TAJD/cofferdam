# cd-9hp.1 — Decision: custom mini-DSL (signed off 2026-05-20)

User accepted the recommendation. Parser implementation proceeds against `docs/dsl-grammar.md`.

## Checkpoint 1: grammar spec (shipped 2026-05-20, commit 30456f3)

`docs/dsl-grammar.md` is the contract. Any deviation in the parser is a parser bug, not license to amend the spec. Grammar changes go through a MAJOR/MINOR bump per `docs/schema-versioning.md`.

## Kickoff prompt for fresh session — checkpoint 2 (parser + AST)

Copy-paste verbatim into a fresh Claude Code session:

---

Implement the cd-9hp.1 v1 predicate DSL parser per `docs/dsl-grammar.md`. The grammar spec is the contract; deviations are bugs. This is checkpoint 2 of 5.

Scope:

1. Create `crates/cofferdam-core/src/dsl/` (new module). Files: `mod.rs`, `ast.rs`, `parser.rs`, `tests.rs`.
2. AST types (`ast.rs`) — Rust enums matching the EBNF in the spec: `Predicate`, `Comparison`, `Subject`, `Op`, `Operand`, `Call`. Lock the shapes to what the grammar admits today; reserved-for-v2 surfaces (quantifiers, aggregation) are NOT in the AST.
3. Pratt parser (`parser.rs`) — recursive descent with operator precedence. Boolean ops `or` / `and` / `not` left-to-right with `not` highest precedence; comparisons under boolean ops; `+` as a string-concat operator inside `operand`. Targets the exact `predicate` production from the grammar.
4. Error reporting — `DslParseError` enum with `MalformedToken { line, col, msg }`, `UnknownSubject { name, suggestions }`, `UnknownOperator { name, suggestions }`, `UnregisteredNamespace { ns, known }`, `BadStringEscape { line, col }`. Each error carries a 1-based location so config-load errors point at the right line in `cofferdam.invariants.toml`.
5. Tests (`tests.rs`) — every production path:
   - happy-path: parse each of the 8 operators and 3 functions from the spec, single example each
   - parens: `(a and b) or c` vs `a and (b or c)` — different ASTs
   - precedence: `not a and b` parses as `(not a) and b`
   - subject namespaces: `core.symbol(X)`, `ts.declaration(X)` succeed; `sql.column` errors with the "unregistered namespace" message and suggests the known set
   - operator typos: `imprts` → suggests `imports` (Levenshtein-1)
   - string literals: both single and double-quoted; escape handling for embedded quote chars
   - the two complete examples from the spec round-trip (parse → unparse → parse)

Do NOT in this checkpoint:
- Build the evaluator (checkpoint 3).
- Wire into `Design.ScriptedInvariant` (checkpoint 4).
- Touch any production code outside `crates/cofferdam-core/src/dsl/`.

Verification block before commit: `cargo build --workspace`, `cargo test -p cofferdam-core dsl`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`.

Commit message: `feat(dsl): predicate parser + AST (cd-9hp.1 checkpoint 2)`.

---

## Remaining checkpoints

3. **Evaluator** over flat corpus (~500 LOC). Resolves subjects, walks AST, returns bool/string.
4. **`Design.ScriptedInvariant` integration** + 2 spec_contract fixtures (file-level + cross-file).
5. **Real-repo validation** against bestefforttools and gistreact; docs/invariants.md updated.
