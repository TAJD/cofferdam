# cofferdam

> A watertight compartment for your codebase. Isolate bad code, measure it against rules, ship a priority-sorted verdict.

`cofferdam` is a code-quality analyzer for TypeScript, inspired by Elixir's [Credo](https://github.com/rrrene/credo). The name comes from naval architecture: a cofferdam is a sealed compartment that lets crews work safely below the waterline. The metaphor maps to the tool: keep questionable code isolated, measure it, and surface a prioritised list of what to fix first.

## Why another linter

Cofferdam exists to bring the things Credo got right to the TypeScript world, then improve on them:

1. **Baselines.** Adopt on a legacy codebase without drowning in noise — default mode shows only *new* findings.
2. **Priority and severity are separate axes.** Priority is computed; severity is configured. Sort the report by what to fix first; gate CI on what must not regress.
3. **First-class autofix**, including `cofferdam fix --interactive` for one-at-a-time review.
4. **Type-aware checks** as a first-class tier, not a bolt-on.
5. **Project-graph checks** — call graph, context boundaries, orphaned exports.
6. **Real LSP server** for editor integration.
7. **CI ergonomics** — SARIF, `--since main` for PR-only mode, GitHub annotations.
8. **Configurable taxonomy** — projects can promote Security or Performance to first-class categories.
9. **Speed.** Target < 2s pass-1 on 100k LOC across 8 cores.

## Architecture

Two-tier:

- **Tier 1 (Rust):** engine, AST via [oxc](https://github.com/oxc-project/oxc), project graph via `oxc_resolver`, parallel runner via rayon, baseline diffing, priority computation, formatters, CLI, LSP server via `tower-lsp`.
- **Tier 2 (TS):** type-aware checks via `ts-morph`, plus user plugins loaded into Node `worker_threads` through napi-rs.

The Credo five categories — **Consistency**, **Design**, **Readability**, **Refactor**, **Warning** — are preserved.

## Install

### From source (any platform with Rust)

```bash
cargo install --git https://github.com/TAJD/cofferdam --locked cofferdam-cli
```

This puts `cofferdam` in `~/.cargo/bin`. Requires Rust 1.78+.

### From a release binary (when published)

Each tagged release publishes prebuilt binaries under [GitHub Releases](https://github.com/TAJD/cofferdam/releases). Download the archive for your platform, extract, and put `cofferdam` on your `$PATH`.

| Platform                      | Archive                                                |
| ----------------------------- | ------------------------------------------------------ |
| Linux x64 (glibc)             | `cofferdam-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`     |
| Linux arm64 (glibc)           | `cofferdam-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz`    |
| Linux x64 (musl, alpine etc.) | `cofferdam-vX.Y.Z-x86_64-unknown-linux-musl.tar.gz`    |
| Linux arm64 (musl)            | `cofferdam-vX.Y.Z-aarch64-unknown-linux-musl.tar.gz`   |
| macOS x64                     | `cofferdam-vX.Y.Z-x86_64-apple-darwin.tar.gz`          |
| macOS arm64                   | `cofferdam-vX.Y.Z-aarch64-apple-darwin.tar.gz`         |
| Windows x64                   | `cofferdam-vX.Y.Z-x86_64-pc-windows-msvc.zip`          |

### Cutting a manual release (maintainers)

```pwsh
# Bump version in Cargo.toml first, commit, then:
pwsh scripts/release.ps1 -Tag v0.1.0
```

The PowerShell helper builds a Windows binary, tags, pushes, and creates a GitHub Release with the artifact attached. Full multi-platform builds happen automatically when the tag arrives at the `release.yml` workflow on GitHub.

## Status

**Phase 1, in progress.** What works today:

```bash
cofferdam check src/                          # human-readable report
cofferdam check src/ --robot                  # machine-readable JSON for AI agents
cofferdam check src/ --robot --pretty         # same, indented
cofferdam check                               # walk current dir
cofferdam hello                               # banner
```

Built-in checks live across all five Credo categories:

| ID                              | Category    | What it flags                                      |
| ------------------------------- | ----------- | -------------------------------------------------- |
| `Readability.MaxLineLength`     | Readability | Lines over 120 characters                          |
| `Readability.MaxFunctionLength` | Readability | Function bodies over 50 lines                      |
| `Design.MaxParameters`          | Design      | Functions with > 5 parameters                      |
| `Warning.TripleEquals`          | Warning     | Use of `==` / `!=` instead of `===` / `!==`        |
| `Warning.ParseError`            | Warning     | Files oxc couldn't parse                           |

Plus stubs for `Consistency.QuoteStyle` and `Refactor.CognitiveComplexity` — landing as real checks in upcoming releases.

See `Cargo.toml` for the workspace layout.

### Local toolchain notes

`rust-toolchain.toml` pins the channel to `stable` (host-portable). On a Windows host without the MSVC C++ workload, override the host triple before running cargo:

```pwsh
# PowerShell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
```

```bash
# bash / zsh
export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu
```

Linux and macOS pick up their native host triple and need no override.

## Phased build

1. Rust engine + `Issue` + priority + report formatter + 5 built-in checks across all 5 categories
2. Two-pass consistency mode with `Consistency.QuoteStyle` as the canary
3. Baseline + severity-axis + `--since` ← biggest adoption-unlock
4. napi-rs FFI + JS plugin host, ship `@cofferdam/check-sdk` and `@cofferdam/recommended`
5. `@cofferdam/types-aware` package with `ts-morph` checks
6. LSP server + SARIF + GitHub Code Scanning

## Licence

MIT.
