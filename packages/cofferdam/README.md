# cofferdam

TypeScript code-quality analyzer with a Rust core. Sorts findings by
priority across five categories (Consistency, Design, Readability,
Refactor, Warning) and gates CI on a configurable severity axis.
Inspired by Elixir's [Credo](https://github.com/rrrene/credo).

## Install

```bash
pnpm add -D cofferdam        # pnpm
npm install -D cofferdam     # npm
yarn add -D cofferdam        # yarn
```

The package downloads a pre-built binary for your platform on install
(Linux x64/arm64 gnu+musl, macOS x64/arm64, Windows x64) and runs it
through a tiny JS shim.

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

| Check ID                          | Category    | Default severity | Default limit |
|-----------------------------------|-------------|------------------|---------------|
| `Warning.TripleEquals`            | Warning     | high             | —             |
| `Refactor.CyclomaticComplexity`   | Refactor    | medium           | 10            |
| `Refactor.CognitiveComplexity`    | Refactor    | medium           | 15            |
| `Refactor.DuplicateBlock`         | Refactor    | medium           | —             |
| `Design.MaxParameters`            | Design      | medium           | 5             |
| `Design.DuplicateExportName`      | Design      | medium           | —             |
| `Readability.MaxLineLength`       | Readability | low              | 120           |
| `Readability.MaxFunctionLength`   | Readability | low              | 50            |
| `Consistency.QuoteStyle`          | Consistency | low              | —             |

Severity gates CI; priority sorts the report. The two are deliberately
separate axes — see [the project repo](https://github.com/TAJD/cofferdam)
for the design rationale and the full check catalog.

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
working directory until a `.git` is reached. Every key is optional —
unset values fall back to the defaults above.

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

Two inline directive forms; both accept an optional comma-separated
list of check IDs to scope the silencing.

```ts
// cofferdam-disable-next-line Warning.TripleEquals
if (a == b) { /* legitimate type-coercion comparison */ }

/* cofferdam-disable Refactor.CyclomaticComplexity */
function legacyDispatcher(event) {
  // big switch statement — refactor is tracked separately
}
/* cofferdam-enable */
```

Without an ID list, `disable-next-line` silences every check on the
next non-blank line; an unmatched `cofferdam-disable` block extends to
end of file.

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
   `npm rebuild cofferdam`.
2. **Pre-baked image** — set `COFFERDAM_SKIP_DOWNLOAD=1` if the
   binary is already at `node_modules/cofferdam/bin/cofferdam`
   (or `cofferdam.exe` on Windows).

> **Windows + npm 6:** bare `npx cofferdam` falls back to `npm run` and
> fails with `Missing script: 'cofferdam'`. Use `npx -p cofferdam cofferdam`,
> `pnpm exec cofferdam`, or `.\node_modules\.bin\cofferdam.cmd`, or
> upgrade to npm ≥ 7.

## Versioning

The npm package version tracks the cofferdam release version.
`cofferdam@0.1.0` downloads the binary from the `v0.1.0` GitHub
Release. Lockfile-pinned installs are deterministic.

## License

MIT. Full source and project documentation:
[github.com/TAJD/cofferdam](https://github.com/TAJD/cofferdam).
