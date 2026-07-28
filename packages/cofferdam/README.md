# cofferdam

TypeScript code-quality analyzer with a Rust core. Sorts findings by
priority across five categories (Consistency, Design, Readability,
Refactor, Warning) and gates CI on a configurable severity axis.
Inspired by Elixir's [Credo](https://github.com/rrrene/credo).

## Install

```bash
pnpm add -D @cofferdam/cofferdam        # pnpm
npm install -D @cofferdam/cofferdam     # npm
yarn add -D @cofferdam/cofferdam        # yarn
```

The package downloads a pre-built binary for your platform on install
(Linux x64/arm64 gnu+musl, macOS x64/arm64, Windows x64) and runs it
through a tiny JS shim.

> **pnpm users:** pnpm v10's default sandbox blocks postinstall scripts
> unless the package is on the allowlist. `pnpm add -D @cofferdam/cofferdam`
> will "succeed" without ever downloading the binary. Add this to your
> `package.json` so the binary install actually runs:
>
> ```json
> { "pnpm": { "onlyBuiltDependencies": ["@cofferdam/cofferdam"] } }
> ```
>
> Then re-run `pnpm install` (or `pnpm rebuild @cofferdam/cofferdam` for
> an existing install). Verified you're hit by this if
> `pnpm exec cofferdam --version` errors with "binary not found".

## First run

```bash
pnpm exec cofferdam init
```

`init` writes three things and asks once whether to capture a baseline
of your current findings:

- **`cofferdam.toml`** — every check stanza commented out so you can
  see what's tunable. Edit only the values you care about; defaults
  cover the rest.
- **`.cofferdam/baseline.json`** — snapshot of findings the build
  should *not* fail on (your existing tech debt). Commit this file.
- **`.gitignore`** — `.cofferdam/` is added with a
  `!.cofferdam/baseline.json` negation, so the baseline stays
  tracked while future cache content is ignored.

After `init`:

```bash
pnpm exec cofferdam check src/        # exits 0 — everything baselined
```

Add a fresh `==` to your code, re-run, and CI will fail with only the
new finding flagged.

## Built-in checks

Cofferdam ships 20+ built-in checks across all five categories,
including project-graph rules (`Design.OrphanExport`,
`Design.ImportCycle`, `Design.LayerViolation`, `Refactor.DeadExport`)
and complexity rules (`Refactor.CyclomaticComplexity`,
`Refactor.CognitiveComplexity`). Severity gates CI; priority sorts
the report — the two are deliberately separate axes.

Full catalog with bad/good examples, options, and per-check defaults:
**<https://tajd.github.io/cofferdam/checks/>**.

## CI integration

### GitHub Actions

```yaml
- uses: actions/checkout@v6
  with:
    # fetch-depth: 0 so --since can resolve the base branch ref
    fetch-depth: 0
- run: pnpm install --frozen-lockfile
- run: pnpm exec cofferdam check src/ --since origin/${{ github.base_ref }} --fail-on=high
```

`--since <git-ref>` runs only against files changed in `<ref>...HEAD`,
so PR checks stay fast on large repos. `--fail-on=high` exits 1 only
when at least one finding is High or Critical — Medium and below
print but don't fail CI.

### Husky pre-commit

```bash
# .husky/pre-commit
pnpm exec cofferdam check --since HEAD --fail-on=high
```

## Configuration

`cofferdam.toml` at the project root. Discovered by walking up from the
path you asked to analyze (`cofferdam check src/app` finds
`src/app/cofferdam.toml`), falling back to the working directory when
that turns up nothing; either walk stops at a `.git`. Every key is
optional — unset values fall back to the defaults.

```toml
# Lower a check's severity so it stops failing CI but still appears in reports
[checks."Refactor.CyclomaticComplexity"]
severity = "low"

# Tighten a limit
[checks."Readability.MaxLineLength"]
limit = 100
```

Override per invocation: `--config <path>` points at a specific file,
`--no-config` skips discovery entirely.

## Suppression

Inline directives let you silence a finding with an auditable reason
field. Canonical form:

```ts
// cofferdam-ignore: Warning.NoEval: codegen bootstrap, not user input
eval(generatedCode);
```

Range and file-scoped variants:

```ts
// cofferdam-ignore-start: Refactor.CyclomaticComplexity
function generatedRouter(req, res) { /* ... */ }
// cofferdam-ignore-end

// cofferdam-ignore-file: Readability.MaxLineLength
```

ESLint-style aliases (`// cofferdam-disable-next-line <CheckId>`,
`/* cofferdam-disable */ ... /* cofferdam-enable */`) are also
recognised for ergonomic continuity. Full syntax and reason-field
rules: <https://tajd.github.io/cofferdam/suppression/>.

## Custom checks

Author project-specific checks in TypeScript with
[`@cofferdam/check-sdk`](https://www.npmjs.com/package/@cofferdam/check-sdk).
The `defineCheck` API gives you AST and line views over the same
sources cofferdam already parsed; the cofferdam binary spawns a Node
host and merges your findings into its stream. Plugin authoring guide:
<https://tajd.github.io/cofferdam/plugin-sdk/> (in progress).

## Architectural specs

Pin the shape of your codebase in `cofferdam.invariants.toml` —
declare layer boundaries (`ui` cannot import `db`), freeze a public
surface (`packages/sdk` exports may not change without a deliberate
update), and let `Design.LayerViolation` and `Design.BoundaryFrozen`
fail CI on drift. Spec reference:
<https://tajd.github.io/cofferdam/invariants/>.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | No findings at or above `--fail-on` (default: `medium`). Baselined findings never trigger the gate. |
| 1    | At least one finding triggers the gate. |
| 2    | Invocation, IO, or config error. |

## Sandboxed installs / `--ignore-scripts`

If your installer disables postinstall scripts, the binary won't be
downloaded. Two recovery paths:

1. **Manual binary** — download the release archive from
   [GitHub Releases](https://github.com/TAJD/cofferdam/releases),
   extract it, set `COFFERDAM_BINARY_PATH` to the binary, then
   `npm rebuild @cofferdam/cofferdam`.
2. **Pre-baked image** — set `COFFERDAM_SKIP_DOWNLOAD=1` if the
   binary is already at `node_modules/@cofferdam/cofferdam/bin/cofferdam`
   (or `cofferdam.exe` on Windows).

> **Windows + npm 6:** bare `npx cofferdam` falls back to `npm run` and
> fails with `Missing script: 'cofferdam'`. Use
> `npx -p @cofferdam/cofferdam cofferdam`, `pnpm exec cofferdam`, or
> `.\node_modules\.bin\cofferdam.cmd`, or upgrade to npm ≥ 7.

## Versioning

The npm package version tracks the cofferdam release version.
`@cofferdam/cofferdam@0.2.3` downloads the binary from the `v0.2.3`
GitHub Release. Lockfile-pinned installs are deterministic. The Rust
workspace and the `@cofferdam/cofferdam` + `@cofferdam/check-sdk` npm
packages are released in lockstep — an `@cofferdam/cofferdam@X.Y.Z`
install always pairs with an SDK at the same `X.Y.Z`.

## License

MIT. Full source and project documentation:
[github.com/TAJD/cofferdam](https://github.com/TAJD/cofferdam).
