# cd-91zc — Decision: phase 0 against flat-corpus shape (signed off 2026-05-20)

User direction: dogfood cofferdam on its own Rust codebase. Two strategic payoffs — polylingual demonstration of the architecture, and concrete pressure on cd-9hp.9 (canonical graph) + cd-9hp.10 (adapter contract). Both briefs were designed in vacuum; the Rust adapter is the load-bearing user that informs their final shape.

`cd-9hp.10` now `depends on cd-91zc` — the adapter contract crystallises from this work.

## Shipped checkpoints

### Checkpoint 1 — crate skeleton + tree-sitter linkage (2026-05-20)

- `crates/cofferdam-rust/` exists, in workspace `members`.
- `Cargo.toml` pins `tree-sitter = "0.23"` and `tree-sitter-rust = "0.23"`.
- `src/lib.rs` documents planned module layout. Smoke test confirms parser links.

### Checkpoint 2 — parser → Span layer (2026-05-20, commit 157e640)

- `crates/cofferdam-rust/src/parser.rs` ships the conversion surface.
- Public API: `parse_rust`, `RustParseTree`, `RustParseError`. `root_node`, `text`, `span_of`, `text_of`, `has_errors`, `error_spans`.
- 7 tests covering byte-range round-trip, UTF-8 sanity, multi-line line/column tracking, and tree-sitter's error-recovery surface.
- `RustParseTree::has_errors` / `error_spans` added beyond the original scope — tree-sitter is recovery-oriented so checks need an "is this trustworthy?" gate.

## Kickoff prompt for fresh session — checkpoint 3 (`Rust.NoUnwrapInLib`)

Copy-paste verbatim into a fresh Claude Code session:

---

Implement `Rust.NoUnwrapInLib` for cd-91zc phase 0. The parser layer (`cofferdam_core::span_from_bytes` + `cofferdam-rust`'s `parse_rust` / `RustParseTree`) is shipped — see `crates/cofferdam-rust/src/parser.rs`. This is checkpoint 3 of 6.

Scope:

1. Create `crates/cofferdam-rust/src/checks/mod.rs` (re-exports a future `all_rust_checks()`) and `crates/cofferdam-rust/src/checks/no_unwrap_in_lib.rs`.

2. The check, by example:

   ```rust
   // FIRES:
   let x = some_option.unwrap();
   let y = result.expect("won't happen");

   // DOES NOT FIRE — test context:
   #[cfg(test)]
   mod tests {
       #[test]
       fn it_works() {
           assert_eq!(parse("1").unwrap(), 1);  // ok
       }
   }

   // DOES NOT FIRE — test attribute on the function itself:
   #[test]
   fn standalone_test() {
       compute().unwrap();
   }
   ```

   Heuristic: a call expression where the method identifier is `unwrap` or `expect`, and no ancestor node is:
   - a function/impl/mod with `#[cfg(test)]` attribute, or
   - a function with `#[test]` attribute, or
   - a `mod` named `tests`.

3. Implement `Check` trait from `cofferdam_core::Check` matching the existing built-in pattern (see `crates/cofferdam-checks/src/refactor.rs` for `Refactor.CyclomaticComplexity` as a reference). `CheckMeta`:

   ```rust
   const META: CheckMeta = CheckMeta {
       id: "Rust.NoUnwrapInLib",
       category: Category::Warning,
       base_priority: 12,
       default_severity: Severity::Medium,
       explanation: "...",
       body: include_str!("../../docs/Rust.NoUnwrapInLib.md"),
       requires_types: false,
       consistency: false,
       options: &[],
       autofix: false,
   };
   ```

   Don't add `Rust.NoUnwrapInLib` to `cofferdam-checks::all_builtins()` — that's checkpoint 4 (engine wiring with the `Language` enum). For now the check lives in `cofferdam-rust` and is callable from tests.

4. Per-check doc page at `crates/cofferdam-rust/docs/Rust.NoUnwrapInLib.md`. Same format as the doc pages under `crates/cofferdam-checks/docs/` — frontmatter + prose.

5. Fixtures under `crates/cofferdam-rust/tests/fixtures/no_unwrap_in_lib/`:
   - `flagged_in_lib.rs` — a lib-style file with `.unwrap()` outside test context. Expect 2-3 findings.
   - `silent_in_test_module.rs` — `#[cfg(test)] mod tests { ... }` with `.unwrap()` inside. Expect 0 findings.
   - `silent_with_test_attr.rs` — `#[test] fn ...` with `.unwrap()` inside. Expect 0 findings.
   - `mixed.rs` — both lib and test contexts in one file; expects only lib-context findings.

6. Use `parse_rust` to get the tree; iterate `call_expression` nodes via a `TreeCursor`. For each, walk back up the ancestor chain checking for the three test markers (`cfg(test)` attr, `#[test]` attr, `mod tests`).

7. Tests use plain assertions over `Vec<Issue>`. Match the patterns in `crates/cofferdam-checks/src/refactor.rs`'s test modules.

Do NOT in this checkpoint:
- Add the `Language` enum to `SourceFile` (checkpoint 4).
- Wire into `cofferdam-engine` / `cofferdam-cli` (checkpoint 4).
- Ship `Rust.NoUnimplementedInNonTest` or `Rust.MissingPubDoc` (checkpoint 5).
- Touch `cofferdam-checks::all_builtins()`.

Verification block: `cargo build -p cofferdam-rust`, `cargo test -p cofferdam-rust`, `cargo clippy -p cofferdam-rust --all-targets -- -D warnings`, `cargo fmt --check`.

Commit message: `feat(cofferdam-rust): Rust.NoUnwrapInLib check (cd-91zc checkpoint 3)`.

---

## Remaining checkpoints

3. **`Rust.NoUnwrapInLib`** — first concrete check (this checkpoint).
4. **Engine wiring** — `Language` enum on `SourceFile`; per-file dispatch routes to Rust checks when `Language::Rust`. `cofferdam check crates/` works end-to-end.
5. **Two more checks** — `Rust.NoUnimplementedInNonTest`, `Rust.MissingPubDoc`. Validates the path generalises.
6. **CI dogfood** — `crates/`'s baseline shipped; CI gate parallel to cd-9tq's TS dogfood.

Phase 1 (post-cd-9hp.9): migrate to the canonical graph.
Phase 2 (post-cd-9hp.10): formalise the adapter contract.
