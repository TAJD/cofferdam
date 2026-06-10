# Language support

## TypeScript (primary)

TypeScript is cofferdam's primary surface: every flagship check — orphan exports, layering, complexity, suppression hygiene — targets TS / TSX / JS / JSX / MJS / CJS via the [oxc](https://oxc.rs/) parser.

## Rust (second language, dogfood)

A Rust adapter ships under `crates/cofferdam-rust`, exercising the engine's per-language dispatch (cd-91zc). Three checks today:

- `Rust.NoUnwrapInLib` — no `.unwrap()` in library code
- `Rust.NoUnimplementedInNonTest` — no `unimplemented!()` outside tests
- `Rust.MissingPubDoc` — public items need doc comments

A CI dogfood job runs cofferdam against its own Rust workspace on every PR.

The Rust adapter is load-bearing for the polylingual architecture pledge: until a second domain existed, "framework, not TS-tool" was theory. It now ships as a working second parser feeding the same `Check` trait the TS adapter uses.

## Future adapters

SQL, IaC, and GraphQL adapters follow the same shape — a parser feeding the shared `Check` trait and category model. See the [phased roadmap](https://github.com/TAJD/cofferdam/blob/main/MAINTAINERS.md#phased-build).
