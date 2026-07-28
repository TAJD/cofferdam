# cofferdam doctor

`cofferdam doctor` is a one-stop diagnostic command for your cofferdam installation and project configuration. Run it on a fresh checkout, after upgrading cofferdam, or whenever something feels off. It never edits files — it only inspects and reports.

## Usage

```bash
cofferdam doctor
cofferdam doctor --robot
cofferdam doctor --robot --pretty
```

| Flag | Effect |
|---|---|
| `--robot` | Emit a single-line JSON object instead of human-readable output |
| `--pretty` | Pretty-print JSON (only with `--robot`) |

## Checks

Doctor runs 11 checks in sequence and reports every result before exiting. All 11 always run — there is no early exit on first failure, so you see the complete picture in one shot.

### binary-integrity

Verifies that the running binary's compiled-in version (`env!("CARGO_PKG_VERSION")`) matches what `cofferdam --version` reports at runtime. Passes when both agree. Fails when they differ, which can happen after a partial upgrade where the new binary wasn't written atomically.

| Status | Condition |
|---|---|
| Pass | Versions match |
| Fail | Versions differ, or the executable cannot exec itself |

Remediation: `re-install @cofferdam/cofferdam`

---

### config

Searches for `cofferdam.toml` by walking up from the current directory to the nearest `.git` root. (`cofferdam check` uses the same walk, but anchors it on the paths you ask it to analyze and only falls back to the current directory; `doctor` takes no target path, so it always reports on the current-directory anchor.) Reports whether the file exists, whether it parses cleanly, and whether it references any unknown check IDs.

| Status | Condition |
|---|---|
| Pass | No `cofferdam.toml` found (defaults in effect), or file found and parses cleanly |
| Warn | File parses but contains one or more unknown check IDs |
| Fail | File exists but has a syntax error |

Remediation for Warn: `remove the unknown key(s) or upgrade cofferdam`
Remediation for Fail: `fix the syntax error in <path>`

---

### baseline

Looks for `.cofferdam/baseline.json` by walking up from the current directory. If found, reads and parses it. Additionally checks that every file path referenced by a baselined entry still exists on disk (stale paths indicate the baseline needs refreshing after files were renamed or deleted). A baseline written by an older or newer binary — one whose schema `version` doesn't match what this binary writes — is called out explicitly by name, with the found and supported version numbers, rather than surfacing as a generic parse failure.

| Status | Condition |
|---|---|
| Pass | No baseline file found, or file loads cleanly with no stale entries |
| Warn | Baseline is valid but references one or more files no longer in the repo, or the baseline's schema version doesn't match what this binary writes |
| Fail | File exists but cannot be read or parsed (JSON error) |

Remediation for Warn or Fail: `cofferdam baseline write`

---

### git

Checks that `git --version` exits successfully and that the current working directory is inside a git repository (`git rev-parse --show-toplevel`). Git is required for `--since` (PR-only mode) and CI integrations that gate on changed files.

| Status | Condition |
|---|---|
| Pass | `git` found on PATH and cwd is inside a repo |
| Warn | `git` found but cwd is not inside a repo |
| Fail | `git` not found on PATH |

Remediation for Fail: `install git for --since and CI integration`
Remediation for Warn: `run cofferdam from inside a git checkout if you need --since`

---

### discovery

Runs the default discovery pass against `.` (the same walk `cofferdam check` uses with no explicit paths). Reports how many TypeScript files were found.

| Status | Condition |
|---|---|
| Pass | One or more TypeScript files found, or zero files but `[discovery]` is configured in `cofferdam.toml` |
| Warn | No TypeScript files found and no explicit scope is configured |

Remediation: `scope cofferdam by passing paths or configuring [discovery] in cofferdam.toml`

---

### suppression-directives

Discovers all `.ts`/`.tsx` files (same walk as discovery), then scans each file for `// cofferdam-disable-next-line` and `/* cofferdam-disable */` directives that name specific check IDs. Validates every named ID against the current `all_builtins()` registry. This catches stale directives left over after a check was renamed or removed.

If more than 1000 files are found, the scan is skipped with a warning (use `--paths` to narrow scope and then re-run).

| Status | Condition |
|---|---|
| Pass | All named directive IDs are known |
| Warn | One or more directive IDs reference unknown checks, or >1000 files (scan skipped) |

Remediation: `rename to a current check ID or remove the directive — see cofferdam explain --list`

---

### type-host

Checks whether any registered check has `requires_types: true` (type-aware checks, routed through the ts-morph type host). If none are registered, this check is a no-op pass. If at least one is registered, walks up from the current directory looking for a root `tsconfig.json` — the same discovery `cofferdam check` uses to start the type host. This catches the common pnpm/turbo monorepo footgun: a `tsconfig.base.json` plus per-package `tsconfig.json` files, but nothing at the project root, which silently disables every type-aware check.

| Status | Condition |
|---|---|
| Pass | No type-aware checks registered, or a root `tsconfig.json` was found |
| Warn | Type-aware checks are registered but no `tsconfig.json` was found near the project root |

Remediation for Warn: add a root `tsconfig.json` (a references-only one pointing at the per-package configs is enough) so type-aware checks can run.

---

### wrapper-version

Only meaningful when cofferdam is installed via npm. Detects whether a `packages/cofferdam/package.json` file is present up the path from the current executable (which indicates a source-tree install or an npm-installed binary). Compares the npm package `version` field against the compiled binary version.

If no `packages/cofferdam/package.json` is found, the check is skipped — it does not apply to direct binary installs.

| Status | Condition |
|---|---|
| Pass | Versions match, or no npm package found (skipped) |
| Warn | npm package and binary version differ |
| Skipped | Not an npm install |

Remediation for Warn: `re-run npm install @cofferdam/cofferdam` (for npm users). Version drift in a source-tree checkout is intentional pre-1.0 behaviour.

---

### formatter-coexistence

Looks for a `biome.json`/`biome.jsonc` or an eslint config (`eslint.config.{js,mjs,cjs,ts}`, `.eslintrc*`) in the current directory. If a linter/formatter is present, cofferdam has a handful of built-in checks that overlap rules biome and eslint both ship out of the box — `Consistency.QuoteStyle`, `Warning.TripleEquals`, `Warning.NoConsoleLog`, `Warning.NoDebugger`, `Refactor.PreferNullishCoalescing`, `Refactor.PreferOptionalChain`. Running both tools unmodified means the same line gets flagged twice.

This is informational, not a correctness problem — some teams want the belt-and-suspenders double coverage. `biome.json` takes precedence over an eslint config if both are present (most repos migrating to biome keep a stale `.eslintrc` around during the transition).

| Status | Condition |
|---|---|
| Pass | No `biome.json`/eslint config found |
| Warn | A `biome.json`/eslint config is present |

Remediation for Warn: disable the overlapping checks via `[[overrides]]` blocks in `cofferdam.toml` (`paths = ["**"]`, `disabled = true`, one block per check) — see [Per-path overrides](./overrides.md).

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All checks passed (including warns and skips) |
| 1 | One or more checks failed |

Warnings do **not** cause a non-zero exit. Only `Fail`-status checks trigger exit code 1. This is intentional — pre-flight friendliness means CI can include `cofferdam doctor` as an informational step without making the build brittle.

## JSON schema (`--robot`)

The `--robot` flag emits a single JSON object on one line (or pretty-printed with `--pretty`). The schema is **additive-only** across versions: fields will not be renamed or removed; new fields may be added.

Example (pretty-printed):

```json
{
  "results": [
    {
      "name": "binary-integrity",
      "status": "pass",
      "message": "0.1.4 (matches build)",
      "remediation": null
    },
    {
      "name": "config",
      "status": "warn",
      "message": "cofferdam.toml at /repo/cofferdam.toml — unknown check id(s): Bogus.Whatever",
      "remediation": "remove the unknown key(s) or upgrade cofferdam"
    }
  ],
  "summary": {
    "pass": 6,
    "warn": 1,
    "fail": 0,
    "skipped": 0
  }
}
```

**Field reference:**

| Field | Type | Values |
|---|---|---|
| `results[].name` | string | Check identifier (stable across versions) |
| `results[].status` | string | `"pass"`, `"warn"`, `"fail"`, `"skipped"` |
| `results[].message` | string | Human-readable one-line summary |
| `results[].remediation` | string or `null` | Remediation hint; `null` when status is pass/skipped |
| `summary.pass` | integer | Count of passing checks |
| `summary.warn` | integer | Count of warning checks |
| `summary.fail` | integer | Count of failing checks |
| `summary.skipped` | integer | Count of skipped checks |
