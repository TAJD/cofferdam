# `cofferdam.invariants.toml` — project-wide architectural spec

One canonical artifact for "what is this codebase supposed to be?" — read by
humans, agents, and multiple checks at once. Promotes the per-check
`[layers]` block from `cofferdam.toml` to a shared spec that also covers
public-API allowlisting, frozen boundaries, and arbitrary forbid/require
import rules.

## File location

`cofferdam.invariants.toml` lives next to `cofferdam.toml` at the project
root. Discovery walks up from the working directory until it finds the
file or hits a `.git` entry — same rule as `cofferdam.toml`. Both files
are optional and additive; you can ship one without the other.

## Schema

```toml
schema_version = "1.0"

[layers]
infra  = ["src/infra/**"]
domain = ["src/domain/**"]
app    = ["src/app/**"]

[layers.allow]
domain = ["infra"]
app    = ["domain", "infra"]

[public_api]
exports = ["package.json:exports", "src/index.ts"]

[boundaries]
"src/legacy/**" = { frozen = true, reason = "see ADR-0007" }

[invariants]
"no-direct-db-access" = { forbid_imports = ["src/infra/db"], from_layers = ["app"] }
"telemetry-required"  = { require_imports = ["src/infra/telemetry"], from_layers = ["app"] }
```

### `schema_version`

`MAJOR.MINOR` (semver-flavoured). Accepted as integer (`1` → treated
as `1.0`) or string (`"1.0"`, `"1.2"`). The current version this build
ships is `1.0`. The field is honoured at load time — future versions
the build doesn't understand are rejected with an upgrade message;
past versions outside the deprecation window are rejected with a
migration message. A spec without `schema_version` is loaded as
`1.0` for backwards compatibility, with a one-time hint pointing at
this section.

Full policy — bump rules, deprecation window, supported versions —
lives in [docs/schema-versioning.md](./schema-versioning.md).

### `[layers]` and `[layers.allow]`

Identical shape to the `[layers]` block in `cofferdam.toml`. When both
files declare layers, the invariants spec wins and the CLI emits a
deprecation hint pointing at `cofferdam.toml`. Read by
`Design.LayerViolation`.

### `[public_api]`

`exports` is a list of "entry-point sentinels". Each entry is either:

* a relative path to a TS/JS file (`src/index.ts`) — every export from
  that file is exempt from `Design.OrphanExport`,
* a glob pattern (`components/ui/**/*.tsx`) — every file whose
  project-root-relative path matches the pattern is exempt. Useful for
  vendored UI directories (shadcn/ui, etc.) where a single line covers
  many files. Standard glob metacharacters `*`, `**`, `?`, `[…]`, and
  `{…,…}` are supported; an invalid pattern is silently skipped (the
  check still runs, the pattern just exempts nothing), or
* a `package.json:<key>` pointer (`package.json:exports`) — schema
  accepts it; resolution lands in a follow-up bead.

**Example — exempt a vendored UI directory:**

```toml
[public_api]
exports = [
  "src/index.ts",
  "components/ui/**/*.tsx",
]
```

Read by `Design.OrphanExport`.

### `[boundaries]`

Glob → boundary metadata. `frozen = true` marks the area as off-limits
to new code; v0 stub-warns one finding per file matching the glob
(`Design.BoundaryFrozen`), with `reason` echoed in the message. Per-file
delta enforcement against a baseline lands in a follow-up bead.

### `[invariants]`

Named forbid/require import rules, each fired independently:

* `forbid_imports` — list of project-relative path prefixes (or bare
  specifiers like `lodash`). An import edge whose resolved path or
  source specifier starts with any prefix triggers a finding at the
  import statement.
* `require_imports` — list of prefixes that must be imported by every
  file in `from_layers`. A file with no matching import receives one
  finding at its first import statement.
* `from_layers` — optional layer-name allowlist. When non-empty the
  rule applies only to importing files whose path falls into one of
  those layers (per the merged `[layers]` config). Empty means
  "applies to every in-project file".

Read by `Design.InvariantViolation`. All invariants share one check id
— suppress per-line via `// cofferdam-disable-next-line
Design.InvariantViolation` or globally with a severity override on the
check.

## Migration from `cofferdam.toml`

Existing `[layers]` configuration in `cofferdam.toml` continues to
work. To migrate:

1. Create `cofferdam.invariants.toml` next to `cofferdam.toml`.
2. Move the `[layers]` and `[layers.allow]` blocks across.
3. Add `[public_api]`, `[boundaries]`, or `[invariants]` as needed.
4. Remove `[layers]` from `cofferdam.toml` to silence the deprecation
   hint.

The two files coexist for projects that don't yet need the broader
spec.
