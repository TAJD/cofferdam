# MAINTAINERS

This file covers build setup, toolchain quirks, release procedures, and project internals. End users: see [README.md](README.md).

## Building from source

Requires Rust 1.78+. With [rustup](https://rustup.rs/) installed:

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

### Windows toolchain note

`rust-toolchain.toml` pins the channel to `stable` (host-portable). On a Windows host without the MSVC C++ workload, override the host triple before running cargo:

```pwsh
# PowerShell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
```

```bash
# bash / zsh
export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu
```

Linux and macOS pick up their native host triple and need no override. Never edit `rust-toolchain.toml` to pin a Windows-specific channel — that breaks Linux/macOS CI.

## Binary overrides (npm postinstall)

The `@cofferdam/cofferdam` npm package downloads a prebuilt binary on `postinstall`. Two escape hatches:

- `COFFERDAM_BINARY_PATH=/abs/path` — skip the download and use this binary instead (useful for testing a local build).
- `COFFERDAM_SKIP_DOWNLOAD=1` — skip postinstall entirely (CI / Docker layers where you supply the binary another way).

For air-gapped runners: download the archive for your platform from the GitHub Release, extract it, then point `COFFERDAM_BINARY_PATH` at the extracted binary and run `npm rebuild @cofferdam/cofferdam`.

## Cutting a release

The Rust workspace, the `@cofferdam/cofferdam` npm package, and `@cofferdam/check-sdk` ship in lockstep — every `vX.Y.Z` tag publishes all three at the same version. `release.yml` enforces this on the CI side; locally, use the helper so the bump diff is reviewable in one go.

```bash
# Install once per machine
cargo install cargo-edit --locked

# Bump everything, commit, tag, push
bash scripts/bump-version.sh 0.2.3
git diff -- Cargo.toml packages/        # sanity-check the diff
git commit -am "release: v0.2.3"
git tag v0.2.3 && git push --follow-tags
```

`bump-version.sh` runs `cargo set-version --workspace` (updates `[workspace.package].version` plus every `[workspace.dependencies]` path-dep pin) and `npm version` against both `packages/cofferdam` and `packages/check-sdk`. Single source of truth; no regex-on-TOML.

Once the tag lands on GitHub, `release.yml` builds the multi-platform binaries, publishes the GitHub Release, and runs `publish-npm` + `publish-check-sdk` via OIDC Trusted Publisher.

### Windows-only smoke / dry-run

```pwsh
pwsh scripts/release.ps1 -Tag v0.2.3
```

Builds a Windows binary, tags, pushes, and creates a GitHub Release with that single artifact attached — useful for testing the tag-and-publish flow without the full matrix. Production releases always go through the GitHub Actions matrix.

## npm org & ownership

Both released npm packages live under the `@cofferdam` organisation on npmjs.com:

- `@cofferdam/cofferdam` (binary wrapper) — <https://www.npmjs.com/package/@cofferdam/cofferdam>
- `@cofferdam/check-sdk` (plugin author SDK) — <https://www.npmjs.com/package/@cofferdam/check-sdk>

Both packages publish via OIDC Trusted Publisher pinned to `TAJD/cofferdam` → `release.yml`. If the org's package list ever shows only one of the two, or a publish 401s, the Trusted Publisher config has likely been reset on a transfer — re-confirm the publisher at `npmjs.com/package/<name>/access` points at this exact workflow file before the next release.

The unscoped legacy name `cofferdam` (binary wrapper, owned by user `tajdickson`) was used through 0.2.2 and is now deprecated. New work goes only to the scoped name. The legacy name is kept reserved (npm doesn't release names) so nobody else can squat it; it should not be republished. Migration message users see when they `npm install -D cofferdam` is set with `npm deprecate cofferdam "..."` — see issue cd-gzm for the canonical message.

## Phased build

1. Rust engine + `Issue` + priority + report formatter + built-in checks across all 5 categories
2. Two-pass consistency mode with `Consistency.QuoteStyle` as the canary
3. Baseline + severity-axis + `--since` — biggest adoption-unlock
4. napi-rs FFI + JS plugin host, ship `@cofferdam/cofferdam`, `@cofferdam/check-sdk`, and `@cofferdam/recommended`
5. `@cofferdam/types-aware` package with `ts-morph` checks
6. LSP server + SARIF + GitHub Code Scanning

Phase 1 is in progress. Phases 3–6 are planned.

## Project structure

```
crates/
  cofferdam-core/         # Check trait, Issue, Span, parser, AST surface
  cofferdam-engine/       # discovery, orchestration, parse loop, sort
  cofferdam-checks/       # built-in checks (one file per category)
  cofferdam-formatters/   # text + json output, future compact/SARIF
  cofferdam-cli/          # `cofferdam` binary
  cofferdam-lsp/          # LSP server (phase 6, stub today)
  cofferdam-napi/         # napi-rs FFI surface (phase 4, stub today)
packages/                 # @cofferdam/* npm packages (phase 4+)
examples/                 # fixture .ts files exercised by checks
```

## Real-world corpus

`scripts/fetch-corpus.sh` clones a curated set of TypeScript repos at pinned stable tags into `tests/corpus/` (gitignored, on-demand). The corpus is used by the `corpus_smoke` integration test in `crates/cofferdam-engine/tests/corpus_bench.rs`, which walks each repo, runs the full engine, and writes per-run metrics to `tests/corpus-results/` (also gitignored). Re-running the script is idempotent — already-cloned repos at the expected tag are skipped.

```bash
bash scripts/fetch-corpus.sh          # populate corpus (~couple minutes, network required)
cargo test --test corpus_bench -- --nocapture   # run benchmark
```

To add a new repo to the corpus: append an entry to the three parallel arrays at the top of `fetch-corpus.sh`, verify the tag via `git ls-remote --tags <url>`, and confirm the repo licence is MIT, Apache-2.0, ISC, or BSD before committing.

## Further reading

- Agent / contributor instructions: [CLAUDE.md](CLAUDE.md)
- Check-writing recipe: [CLAUDE.md § Writing a check](CLAUDE.md#writing-a-check-the-recipe)
- Issue tracking (beads): run `bd prime` in the repo root
- CI recipes (user-facing): [docs/ci-recipes.md](docs/ci-recipes.md)
- Check catalog: [docs/checks/](docs/checks/) (auto-generated from `crates/cofferdam-checks/docs/<id>.md` via `cofferdam gen-docs`)
- Output formats: [docs/output-formats.md](docs/output-formats.md)
