# cofferdam

> Your linter checks code after it's written. Cofferdam tells your agent the rules before it writes — and guarantees the debt only goes down.

`cofferdam` is a software architecture and code-quality analyzer for TypeScript with a Rust core. Layer rules, frozen boundaries, import invariants, and complexity budgets are declared once in `cofferdam.invariants.toml`, enforced in CI, and *advised* to AI coding agents just-in-time. Findings are bucketed into five categories — **Consistency**, **Design**, **Readability**, **Refactor**, **Warning** — and priority-sorted within each; the category model is inspired by Elixir's [Credo](https://github.com/rrrene/credo).

```
   advise <file> ↓        ┌────────────────────────────────┐
   advise --diff ↑        │  cofferdam                     │
                           │  layers · invariants ·        │
                           │  boundaries · baseline        │
                           ├────────────────────────────────┤
                           │  tsc                           │
                           ├────────────────────────────────┤
                           │  Biome / ESLint                │
                           ├────────────────────────────────┤
                           │  Biome / Prettier               │
                           └────────────────────────────────┘
```

Cofferdam sits above your linter and type-checker, not instead of them — see
[where cofferdam sits](https://tajd.github.io/cofferdam#where-cofferdam-sits)
for the full picture.

## Documentation

Full docs site: **<https://tajd.github.io/cofferdam>**

- [Check catalog](https://tajd.github.io/cofferdam/checks/) — every built-in check with bad/good examples
- [CLI reference](https://tajd.github.io/cofferdam/reference/cli/) — flags and exit codes
- [`advise` reference](https://tajd.github.io/cofferdam/reference/advise/) — the JSON envelope agents branch on
- [llms.txt](https://tajd.github.io/cofferdam/llms.txt) — the entrypoint for LLM agents: version, subcommands, agent workflow, docs links
- [Install guide](docs/install.md) — binary overrides, air-gapped installs, building from source
- [CI recipes](docs/ci-recipes.md) — GitHub Actions, GitLab, CircleCI, Drone, pre-commit
- [Suppression syntax](docs/suppression.md) — `// cofferdam-ignore` directives
- [Ignore syntax](docs/ignore.md) — `.cofferdamignore` rules
- [Per-path overrides](docs/overrides.md) — retune or disable single checks on matching globs via `[[overrides]]`
- [Type-aware checks](docs/type-aware-checks.md) — checks backed by the TypeScript type system (requires Node + ts-morph; opt out with `[engine] type_aware = false`)
- [Output formats](docs/output-formats.md) — text, JSON, compact, SARIF
- [Architectural specs](docs/invariants.md) — `cofferdam.invariants.toml`, `Design.LayerViolation`, `Design.BoundaryFrozen`
- [Budgets & ratchet](docs/budgets.md) — `[budgets]` hard caps, `baseline ratchet`/`prune`, `check --trend`
- [AI agent workflow](docs/agents.md) — `cofferdam agents` onboarding prompt, plus `agents --hooks`
- [Agent hooks](docs/hooks.md) — wire `advise` into a PreToolUse hook
- [MCP server](docs/mcp.md) — `cofferdam-mcp` exposing advise/check/explain/invariants to agent hosts
- [Doctor](docs/doctor.md) — environment + config diagnostics (`cofferdam doctor`)

## Install

```sh
npm install --save-dev @cofferdam/cofferdam
pnpm add -D @cofferdam/cofferdam
yarn add --dev @cofferdam/cofferdam
```

The `postinstall` script downloads the matching prebuilt binary for your platform (Linux x64/arm64 glibc + musl, macOS x64/arm64, Windows x64). Node 16+ required. Binary overrides, air-gapped installs, and building from source (Rust 1.93+): [install guide](docs/install.md).

## Usage

```sh
$ cofferdam advise src/app/checkout.ts
src/app/checkout.ts
  layer: app          public_api: no
  Design.LayerViolation      imports must target layer(s) [domain, infra]
  Design.InvariantViolation  "no-direct-db-access": must not import src/infra/db
  Design.BoundaryFrozen      not frozen
  Refactor.LongAndComplex    length_limit 75, cyclomatic_limit 15

$ cofferdam advise --diff main
would_fire: 1
  src/app/checkout.ts:12  imports src/infra/db from layer `app`  (Design.InvariantViolation)
would_clear: 0
```

An agent runs `advise` before editing and `advise --diff` before asking for a
commit. For finding-level output on the code as it stands today, `cofferdam
check` gives the same priority-sorted, severity-gated report any linter
does — see the [agent workflow docs](https://tajd.github.io/cofferdam/agents) and
[output formats](https://tajd.github.io/cofferdam/output-formats) for both.

## CI

One command in any runner with Node:

```sh
npx --yes @cofferdam/cofferdam check
```

Ready-made workflows (GitHub Actions, GitLab, CircleCI, Drone, pre-commit), PR-only mode, and baselines: **[`docs/ci-recipes.md`](docs/ci-recipes.md)**.

## Languages

TypeScript (TS / TSX / JS / JSX / MJS / CJS via `oxc`) is the primary surface. A Rust adapter ships as the second language and polylingual proof; SQL, IaC, and GraphQL adapters follow the same shape. Details: [language support](docs/languages.md).

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
