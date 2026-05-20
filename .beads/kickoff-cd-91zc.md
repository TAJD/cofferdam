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
- 7 parser tests covering byte-range round-trip, UTF-8 sanity, multi-line line/column tracking, and tree-sitter's error-recovery surface.

### Checkpoint 3 — `Rust.NoUnwrapInLib` (2026-05-21, commit a98bded)

- `crates/cofferdam-rust/src/checks/no_unwrap_in_lib.rs` ships the check.
- `crates/cofferdam-rust/src/checks/mod.rs` exposes `all_rust_checks()`.
- `crates/cofferdam-rust/docs/Rust.NoUnwrapInLib.md` per-check doc page.
- 4 fixtures + 6 unit tests; check is reachable from tests but NOT yet in `cofferdam-checks::all_builtins()` — that ships with checkpoint 4's engine wiring.
- Key discovery worth carrying forward: `attribute_item` is a **preceding sibling** of the decorated item in tree-sitter-rust, not a child. See `preceding_attribute_items()` for the walking helper.

## Kickoff prompt for fresh session — checkpoint 4 (engine wiring)

Copy-paste verbatim into a fresh Claude Code session:

---

Wire the Rust adapter into the engine for cd-91zc phase 0. The check (`Rust.NoUnwrapInLib`) and parser layer are shipped — see `crates/cofferdam-rust/`. This is checkpoint 4 of 6. Bigger surface than checkpoints 2 and 3; touches cofferdam-core, cofferdam-engine, cofferdam-cli.

Scope:

1. **`cofferdam_core::SourceFile` gains a `language` field.**

   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
   pub enum Language {
       /// TypeScript / TSX / JS / JSX — the existing oxc adapter.
       TypeScript,
       /// Rust — cd-91zc's tree-sitter-rust adapter.
       Rust,
   }
   ```

   Detection at `SourceFile::new` based on file extension: `.ts` / `.tsx` / `.js` / `.jsx` / `.cjs` / `.mjs` → `TypeScript`; `.rs` → `Rust`; default `TypeScript` for backward compatibility (every existing fixture is TS).

2. **`cofferdam-engine`'s per-file run loop dispatches by language.**

   The engine today calls every registered check on every file. Change to: for each file, only invoke checks whose meta declares (or implicitly defaults to) the same language. For phase 0, the simplest cut is a small `language(&self) -> Language` method on `Check` defaulting to `TypeScript`. The Rust adapter's checks override to return `Language::Rust`.

   Adding the method to `Check` is workspace-internal (no plugin SDK impact — plugin SDK is TS-only today). Keep the default `TypeScript` so existing built-ins compile unchanged.

3. **`cofferdam-checks::all_builtins()` includes `Rust.NoUnwrapInLib`.**

   Add `cofferdam-rust = { workspace = true }` to `cofferdam-checks/Cargo.toml`. `all_builtins()` appends `cofferdam_rust::all_rust_checks()`. The engine's per-language dispatch ensures `Rust.NoUnwrapInLib` only runs on `.rs` files.

4. **`cofferdam check crates/`** works end-to-end. Tree-sitter parser runs on every `.rs` file; `Rust.NoUnwrapInLib` fires on unwraps in lib context. Verify against the cofferdam codebase itself — expect findings on real unwraps in `cofferdam-engine` / `cofferdam-cli` (legitimate refactor candidates).

5. **`cofferdam-cli` discovery** accepts `.rs` files. Check `crates/cofferdam-cli/src/main.rs` and `crates/cofferdam-engine/src/discover.rs` — the `ignore::WalkBuilder` likely already picks them up but the per-extension filter may be TS-only. Widen if needed.

6. **`SourceFile::parsed`** (the oxc TS AST) — for `.rs` files this stays `None`. Existing TS checks already gate on `let Some(parsed) = ctx.parsed else { return Vec::new(); };` — they'll skip Rust files cleanly. The Rust check uses its own internal `parse_rust(&file.text)` and ignores `ctx.parsed`.

Do NOT in this checkpoint:
- Add the canonical graph (cd-9hp.9 phase 1).
- Add the adapter contract trait (cd-9hp.10 phase 2).
- Touch the plugin SDK (the SDK is TS-only; Rust plugins are out of scope for phase 0).
- Ship the other two Rust checks (`Rust.NoUnimplementedInNonTest`, `Rust.MissingPubDoc`) — those are checkpoint 5.

Tests / fixtures to add:

- `cofferdam-engine/tests/spec_contract/rust-unwrap-mixed/` — a multi-file fixture (mix of `.rs` lib code + a `tests/` integration test) verifying language dispatch. The existing spec_contract runner needs minor extension to handle `.rs` files; check if `collect_sources` is TS-only and widen.

Verification block: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, plus a manual smoke: `target/debug/cofferdam check crates/cofferdam-rust/src/` should produce findings on any genuine unwrap usage in that crate. Plus the existing TS spec_contract fixtures must still pass — adding the language enum should be additive.

Commit message: `feat(engine): per-language dispatch + Rust adapter wiring (cd-91zc checkpoint 4)`.

---

## Remaining checkpoints after 4

5. **Two more Rust checks** — `Rust.NoUnimplementedInNonTest`, `Rust.MissingPubDoc`. Validates the path generalises.
6. **CI dogfood** — `crates/`'s baseline shipped; CI gate parallel to cd-9tq's TS dogfood.

Phase 1 (post-cd-9hp.9): migrate to the canonical graph.
Phase 2 (post-cd-9hp.10): formalise the adapter contract.
