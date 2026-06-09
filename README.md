# cofferdam

> A watertight compartment for your codebase. Isolate bad code, measure it against rules, ship a priority-sorted verdict.

`cofferdam` is a software architecture and code-quality analyzer for TypeScript with a Rust core. Findings are bucketed into five categories — **Consistency**, **Design**, **Readability**, **Refactor**, **Warning** — and priority-sorted within each. The category model is inspired by Elixir's [Credo](https://github.com/rrrene/credo).

## Documentation

Full docs site: **<https://tajd.github.io/cofferdam>**

- [Check catalog](https://tajd.github.io/cofferdam/checks/) — every built-in check with bad/good examples
- [CLI reference](https://tajd.github.io/cofferdam/reference/cli/) — flags and exit codes
- [CI recipes](docs/ci-recipes.md) — GitHub Actions, GitLab, CircleCI, Drone, pre-commit
- [Suppression syntax](docs/suppression.md) — `// cofferdam-ignore` directives
- [Ignore syntax](docs/ignore.md) — `.cofferdamignore` rules
- [Output formats](docs/output-formats.md) — text, JSON, compact, SARIF
- [Architectural specs](docs/invariants.md) — `cofferdam.invariants.toml`, `Design.LayerViolation`, `Design.BoundaryFrozen`

## Install

```sh
npm install --save-dev @cofferdam/cofferdam
pnpm add -D @cofferdam/cofferdam
yarn add --dev @cofferdam/cofferdam
```

The `postinstall` script downloads the matching prebuilt binary for your platform (Linux x64/arm64 glibc + musl, macOS x64/arm64, Windows x64). Node 16+ required.

Building from source instead requires Rust 1.93+ (enforced in CI): `cargo install --path crates/cofferdam-cli` from a checkout.

## Usage

```sh
$ cofferdam check examples/triple_equals.ts
── Design ───────────────
  [  5] [  medium] examples/triple_equals.ts:3:17  `loose` is exported but never imported in the project  (Design.OrphanExport)
  [  5] [  medium] examples/triple_equals.ts:13:17  `strict` is exported but never imported in the project  (Design.OrphanExport)

── Warning ───────────────
  [ 15] [    high] examples/triple_equals.ts:4:7  use `===` instead of `==`  (Warning.TripleEquals)
  [ 15] [    high] examples/triple_equals.ts:7:7  use `!==` instead of `!=`  (Warning.TripleEquals)

6 finding(s)
```

The first column is the priority score; the second is severity. Priority sorts the report; severity gates CI.

## CI

```yaml
# .github/workflows/cofferdam.yml
name: cofferdam
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      - run: npx --yes @cofferdam/cofferdam check
```

For PR-only mode, baselines, and other CI integrations: see **[`docs/ci-recipes.md`](docs/ci-recipes.md)**.

## Languages

TypeScript is the primary surface: every flagship check (orphan exports, layering, complexity, suppression hygiene) targets TS / TSX / JS / JSX / MJS / CJS via the `oxc` parser.

A second-language **Rust adapter** ships under `crates/cofferdam-rust`, exercising the engine's per-language dispatch (cd-91zc). Three checks today — `Rust.NoUnwrapInLib`, `Rust.NoUnimplementedInNonTest`, `Rust.MissingPubDoc` — and a CI dogfood job that runs cofferdam against its own Rust workspace. The Rust adapter is load-bearing for the polylingual architecture pledge: until a second domain existed, "framework, not TS-tool" was theory; it now ships as a working second parser feeding the same `Check` trait the TS adapter uses.

Future adapters (SQL, IaC, GraphQL) follow the same shape — see [`MAINTAINERS.md`](MAINTAINERS.md#phased-build) for the roadmap.

## Dogfood

Cofferdam runs against its own source on every PR (cd-9tq, cd-91zc):

- **TS SDK** — `packages/check-sdk/src/` is scanned by the `dogfood` job in [`.github/workflows/ci.yml`](.github/workflows/ci.yml). The repo-root [`cofferdam.invariants.toml`](cofferdam.invariants.toml) declares the SDK as `public_api` so leaf-package re-exports aren't flagged as orphans; the three legitimate complexity findings on `plugin-host.ts` ride in [`.cofferdam/baseline.json`](.cofferdam/baseline.json) and are tracked separately. CI fails on any new finding at `--fail-on=high`.
- **Rust workspace** — the `dogfood-rust` job in the same workflow scans every workspace `src/` against [`.cofferdam/baseline-rust.json`](.cofferdam/baseline-rust.json).

To run the same gate locally before pushing:

```sh
cargo build --release -p cofferdam-cli
./target/release/cofferdam check packages/check-sdk/src \
    --baseline .cofferdam/baseline.json --fail-on=high
```

## Status

Phase 4, in progress. See [MAINTAINERS.md](MAINTAINERS.md#phased-build) for the phased roadmap.

## Licence

MIT.
