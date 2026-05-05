# cofferdam

> A watertight compartment for your codebase. Isolate bad code, measure it against rules, ship a priority-sorted verdict.

Maintainers / contributors: see [MAINTAINERS.md](MAINTAINERS.md).

`cofferdam` is a software architecture and code-quality analyzer for TypeScript. The name comes from naval architecture: a cofferdam is a sealed compartment that lets crews work safely below the waterline. The metaphor maps to the tool: keep questionable code isolated, measure it, and surface a prioritised list of what to fix first.

The category model and several design choices are inspired by **[Credo](https://github.com/rrrene/credo)**, the Elixir static analyzer by [@rrrene](https://github.com/rrrene). If you've used Credo, the category names and report shape will feel familiar.

## Why another linter

Cofferdam exists to bring those ideas to TypeScript and then improve on them:

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
- **Tier 2 (TS):** user plugins authored via [`@cofferdam/check-sdk`](packages/check-sdk/) (`defineCheck`, AstView/LineView, Pattern A/B/C surfaces). The cofferdam binary spawns a Node host as a subprocess, hands it parsed line views and a flat AST wire, and merges plugin findings into the engine's stream. Type-aware checks via `ts-morph` are still on the roadmap.

Findings are bucketed into five categories — **Consistency**, **Design**, **Readability**, **Refactor**, **Warning** — and priority-sorted within each.

## Install

```sh
npm install --save-dev cofferdam
# or
pnpm add -D cofferdam
# or
yarn add --dev cofferdam
```

The `postinstall` script downloads the matching prebuilt binary for your platform (Linux x64/arm64 glibc and musl, macOS x64/arm64, Windows x64). Node 16+ required. For air-gapped environments or custom binary paths, see [MAINTAINERS.md](MAINTAINERS.md#binary-overrides).

## Usage

### Basic commands

```sh
cofferdam check src/           # human-readable report
cofferdam check src/ --robot   # machine-readable JSON for AI agents / tooling
cofferdam check                # walk current directory
cofferdam explain Warning.TripleEquals        # prose explanation for a check
cofferdam explain Warning.TripleEquals --robot  # same, as JSON
cofferdam init --baseline      # scaffold cofferdam.toml + capture current state as baseline
cofferdam baseline write       # refresh baseline after fixing findings
```

### Key flags

| Flag | Purpose |
|---|---|
| `--robot` | Default to machine-readable output. Pairs with `--format=compact` for AI agents. |
| `--format=<text\|json\|compact>` | Output format. `text` for humans, `json` for tools, `compact` for AI agents. |
| `--pretty` | Pretty-print JSON output (only with `--format=json` / `--robot`). |
| `--baseline=<path>` | Active baseline file. Auto-detected at `.cofferdam/baseline.json` when present. |
| `--no-baseline` | Disable baseline detection entirely for this run. |
| `--fail-on=<level>` | Severity threshold for exit-1 gate. `info` / `low` / `medium` / `high` / `critical`. Default `medium`. |
| `--fail-on-new` | Only fail on findings absent from the baseline. Implicit when a baseline is active. |
| `--since=<git-ref>` | PR-only mode — only check files changed in `<git-ref>...HEAD`. |
| `--max-issues=<N>` | Cap rendered findings (gate still uses the full set). |
| `--quiet` | Suppress informational output; findings, warnings, and errors still print. |

### Top three CI / agent flows

**1. Fail the build on new findings (GitHub Actions)**

```yaml
# .github/workflows/cofferdam.yml
name: cofferdam
on:
  push:
    branches: [main]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npx --yes cofferdam check
```

`npx --yes cofferdam check` walks the current directory and exits 1 on any finding at `medium` severity or higher. No config required.

**2. PR-only mode — check only changed files**

```yaml
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0   # cofferdam needs full history to diff against the base
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npx --yes cofferdam check --since=origin/${{ github.base_ref }}
        if: github.event_name == 'pull_request'
      - run: npx --yes cofferdam check
        if: github.event_name == 'push'
```

**3. AI-agent / robot mode**

```sh
npx cofferdam check --robot --format=compact
```

Compact format is pipe-delimited (`splitn(8, '|')`-friendly) and ~44% smaller than JSON. Ideal for shovelling findings into an LLM prompt. For full-fidelity JSON (with baseline tags and related spans):

```sh
npx cofferdam check --robot --format=json
```

Full CI recipes for GitHub Actions, GitLab, CircleCI, Drone, and pre-commit hooks: **[`docs/ci-recipes.md`](docs/ci-recipes.md)**.

## Checks

Built-in checks live across all five categories. Full catalog with bad/good examples, options, and suppression directives: **<https://tajd.github.io/cofferdam/checks/>** (or browse the source files at [`docs/checks/`](docs/checks/)).

| ID                                    | Category    | Severity | What it flags                                               |
|---------------------------------------|-------------|----------|-------------------------------------------------------------|
| `Warning.TripleEquals`                | Warning     | high     | `==` / `!=` instead of `===` / `!==`                       |
| `Warning.NoEval`                      | Warning     | high     | `eval(...)` and `new Function(...)`                         |
| `Warning.NoDebugger`                  | Warning     | medium   | `debugger` statements                                       |
| `Warning.NoConsoleLog`                | Warning     | low      | `console.*` calls                                           |
| `Warning.ParseError`                  | Warning     | critical | Files that could not be parsed                              |
| `Refactor.DuplicateBlock`             | Refactor    | medium   | Runs of repeated statements across files (copy-paste)       |
| `Refactor.CyclomaticComplexity`       | Refactor    | medium   | Functions with cyclomatic complexity > 10                   |
| `Refactor.CognitiveComplexity`        | Refactor    | medium   | Functions with cognitive complexity > 15                    |
| `Refactor.PreferOptionalChain`        | Refactor    | low      | `a && a.b` rewritable as `a?.b`                             |
| `Refactor.PreferNullishCoalescing`    | Refactor    | low      | `x \|\| default` rewritable as `x ?? default`              |
| `Design.DuplicateExportName`          | Design      | medium   | Same name exported from multiple files                      |
| `Design.MaxParameters`                | Design      | medium   | Functions with > 5 parameters                               |
| `Readability.MaxFunctionLength`       | Readability | low      | Function bodies over 50 lines                               |
| `Readability.MaxLineLength`           | Readability | low      | Lines over 120 characters                                   |

`Consistency.QuoteStyle` is registered as a stub today; the real implementation lands with two-pass mode.

Output formats reference (text / JSON / compact pipe-delimited): **[`docs/output-formats.md`](docs/output-formats.md)**.

Suppression directives (inline `// cofferdam-ignore` syntax, ESLint-style aliases, reason field): **[`docs/suppression.md`](docs/suppression.md)**.

### Ignoring files

cofferdam reads `.cofferdamignore` at the repo root using gitignore syntax (including negation with `!`). Use it to exclude vendored or generated code from analysis without changing your `.gitignore`:

    node_modules/
    dist/
    *.gen.ts
    !src/special.gen.ts   # un-ignore a single generated file

See [`docs/ignore.md`](docs/ignore.md) for full syntax and precedence rules.

## Status

**Phase 4, in progress.** The Rust engine, all five categories of built-in checks (incl. cross-file project-graph rules — `Design.OrphanExport`, `ImportCycle`, `LayerViolation`, `Refactor.DeadExport`), the CLI with text/JSON/compact/SARIF formats, baseline diffing, PR-only mode (`--since`), two-pass consistency mode, suppression directives (Biome + ESLint syntaxes), `cofferdam.invariants.toml` architectural specs, autofix POC, and the `@cofferdam/check-sdk` plugin SDK with Node-host subprocess loader all work today.

What's coming: type-aware checks via `ts-morph`, an LSP server, MCP tool surface (`cofferdam-mcp`), and `cofferdam advise --diff` for pre-flight validation. See [MAINTAINERS.md](MAINTAINERS.md#phased-build) for the phased roadmap.

## Licence

MIT.
