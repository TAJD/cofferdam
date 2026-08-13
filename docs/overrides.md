# Per-path overrides

`[[overrides]]` blocks in `cofferdam.toml` scope a single check's configuration to files matching a path glob, while every **other** check keeps running on those files. They sit between the two coarser tools:

- `.cofferdamignore` excludes files from **all** checks (see [Ignoring files](/ignore)).
- inline `// cofferdam-ignore` directives suppress findings **per file or per line**, applied by hand (see [Suppression directives](/suppression)).

A `[checks."X"]` block, by contrast, configures a check **globally**. Overrides fill the gap: relax or retune one check for a path pattern without silencing it everywhere or annotating every file.

The canonical motivating case: test files (`**/*.test.ts(x)`) legitimately have long, branchy `describe`/`it` callbacks, so `Refactor.LongAndComplex` flags them as noise — but you still want layer rules, unused-import detection, and everything else running on those files.

---

## Schema

Each `[[overrides]]` block has a `paths` array of globs and a `[overrides.checks."Category.Name"]` sub-table per check it retunes:

```toml
# Global defaults still apply everywhere unless an override changes them.
[checks."Refactor.LongAndComplex"]
length_limit = 75

[[overrides]]
paths = ["**/*.test.ts", "**/*.test.tsx"]

# Relax the length limit for test files...
[overrides.checks."Refactor.LongAndComplex"]
length_limit = 400

# ...and turn off orphan-export detection on them entirely.
[overrides.checks."Design.OrphanExport"]
disabled = true

[[overrides]]
paths = ["src/legacy/**"]
[overrides.checks."Refactor.NearDuplicateBlock"]
severity = "info"
```

An `[overrides.checks."X"]` table accepts:

| Key | Effect |
|---|---|
| any check option (e.g. `limit`) | Overlaid on the global option bag for matching files. Validated against the check's schema at load time, exactly like a `[checks."X"]` block. |
| `severity` | `"info"`, `"low"`, `"medium"`, `"high"`, or `"critical"` — the severity stamped on this check's findings for matching files. |
| `disabled` | `true` skips the check entirely for matching files; `false` re-enables it (so a later block can undo an earlier one). |

---

## Path matching

Globs are written **project-root-relative** in forward-slash form and matched with the same engine as [`[public_api].exports`](/invariants): `*` matches within a path segment, `**` matches across directory separators. The project root is the directory containing `cofferdam.toml`.

```toml
paths = ["**/*.test.tsx"]   # any test file at any depth
paths = ["src/legacy/**"]   # everything under src/legacy
paths = ["src/api/*.ts"]    # direct children of src/api only
```

A path is matched if it matches **any** glob in the block's `paths` array.

---

## Precedence

Overrides cascade. The global `[checks."X"]` value (or the check's built-in default) is the base; each `[[overrides]]` block whose `paths` match the file is applied in **declaration order**, and the **last matching block wins** per key.

```toml
[[overrides]]
paths = ["src/**"]
[overrides.checks."Refactor.LongAndComplex"]
length_limit = 200

[[overrides]]
paths = ["**/*.test.tsx"]
[overrides.checks."Refactor.LongAndComplex"]
length_limit = 400
```

A file at `src/Lobby.test.tsx` matches both blocks; the second is declared later, so its `length_limit = 400` wins. Reorder the blocks to flip the precedence.

This is the same mental model as ESLint's `overrides` array or cascading `.gitignore` rules: order is meaningful, later entries refine earlier ones.

---

## What overrides do *not* touch

- **Other checks keep running.** Disabling or relaxing one check on a glob leaves every other check fully active on those files — that's the whole point.
- **`[layers]`, `[boundaries]`, `[invariants]`, `[public_api]`** are project-graph rules with their own path scoping; overrides target per-check options/severity, not those blocks.
- **Inline suppressions and `.cofferdamignore`** are unaffected and compose normally: a file excluded by `.cofferdamignore` is never analyzed, so no override applies to it.
