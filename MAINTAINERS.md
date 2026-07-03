# MAINTAINERS

This file covers build setup, toolchain quirks, release procedures, and project internals. End users: see [README.md](README.md).

## Building from source

Requires Rust 1.93+ (the declared MSRV, enforced by the CI `msrv` job). With [rustup](https://rustup.rs/) installed:

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

### Windows toolchain note

`rust-toolchain.toml` pins the channel to `stable` (host-portable). On Windows, prefer the default MSVC toolchain. If the MSVC C++ workload is installed, do **not** set a GNU `RUSTUP_TOOLCHAIN` override — mixing GNU-built artifacts with MSVC-built ones (including git hooks' cargo runs) fails with LNK1103 link errors. The GNU override (`RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`) is a last resort only for hosts that cannot install the MSVC workload, and must be used consistently for every cargo invocation on that host.

Linux and macOS pick up their native host triple and need no override. Never edit `rust-toolchain.toml` to pin a Windows-specific channel — that breaks Linux/macOS CI.

## Binary overrides (npm postinstall)

User-facing install mechanics — `COFFERDAM_BINARY_PATH`, `COFFERDAM_SKIP_DOWNLOAD`, air-gapped installs — live in [docs/install.md](docs/install.md) (published on the docs site).

## Cutting a release

The Rust workspace, the `@cofferdam/cofferdam` npm package, and `@cofferdam/check-sdk` ship in lockstep — every `vX.Y.Z` tag publishes all three at the same version. `release.yml`'s `verify` job enforces this before building (it runs `version.mjs check <tag>` and fails the release if the repo doesn't match the tag). Locally, use the deterministic script so the bump diff is reviewable in one go.

```bash
# Bump every version location + regenerate Cargo.lock and checks.json
node scripts/version.mjs set 0.3.4 --regen
node scripts/version.mjs check 0.3.4    # confirm all locations agree
git diff                                # sanity-check the diff
git commit -am "release: v0.3.4"
git tag -a v0.3.4 -m "v0.3.4 — ..." && git push --follow-tags
```

`scripts/version.mjs` is self-contained (no `cargo-edit` dependency). `set` rewrites `[workspace.package].version`, **every** `cofferdam-*` path-dep pin across all crate `Cargo.toml` files (not just `[workspace.dependencies]` — `cofferdam-cli` pins `cofferdam-lsp` directly), and both `package.json` files; `--regen` then refreshes `Cargo.lock` and `docs/public/checks.json`. `check` audits all of them and (with a version arg) asserts they match the tag. The older `bump-version.sh` is a thin wrapper around it. The full sequence, including the v0.3.3 silent-drift post-mortem, lives in `.claude/skills/cut-release`.

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

1. Rust engine + `Issue` + priority + report formatter + built-in checks across all 5 categories — **shipped**
2. Two-pass consistency mode with `Consistency.QuoteStyle` as the canary — **shipped**
3. Baseline + severity-axis + `--since` — biggest adoption-unlock — **shipped**
4. napi-rs FFI + JS plugin host, ship `@cofferdam/cofferdam`, `@cofferdam/check-sdk`, and `@cofferdam/recommended` — **in progress** (plugin host + both npm packages shipped; napi FFI and `@cofferdam/recommended` outstanding)
5. `@cofferdam/types-aware` package with `ts-morph` checks — type-host infrastructure + first type-aware check shipped early (cd-9hp.2); standalone package outstanding
6. LSP server + SARIF + GitHub Code Scanning — SARIF and a workspace-aware LSP shipped early; Code Scanning integration outstanding

## Project structure

The canonical, kept-current crate map lives in [CLAUDE.md § Project structure](CLAUDE.md#project-structure) — one source of truth, used by both humans and agents.

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
- Issue tracking: Cofferdam project (key `CD`) on Projektor
- CI recipes (user-facing): [docs/ci-recipes.md](docs/ci-recipes.md)
- Check catalog: [docs/checks/](docs/checks/) (auto-generated from `crates/cofferdam-checks/docs/<id>.md` via `cofferdam gen-docs`)
- Output formats: [docs/output-formats.md](docs/output-formats.md)
