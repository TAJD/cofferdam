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

## Starter spec

The minimal viable spec for a typical Next.js/Vite app — copy, adjust the
globs, delete what you don't need yet:

```toml
schema_version = "1.0"

[layers]
domain = ["src/domain/**"]
app    = ["src/app/**"]

[layers.allow]
app = ["domain"]

[public_api]
exports = ["src/index.ts"]
```

Ten lines: one layer boundary (`app` may use `domain`, not the reverse) and
one public-API entry point. `[boundaries]` and `[invariants]` are additive —
add them once you have a specific area to freeze or a specific import to
ban.

## Invariants anatomy

Each table maps to exactly one check — this is the reading map an agent
uses to go from spec to enforcement:

```toml
schema_version = "1.0"                        # loader: accepted MAJOR.MINOR

[layers]                                       # ─┐
infra  = ["src/infra/**"]                      #  │ read by
domain = ["src/domain/**"]                     #  ├─► Design.LayerViolation
app    = ["src/app/**"]                        #  │   (which layer a file is in)
                                                # ─┘
[layers.allow]                                 # ─┐
domain = ["infra"]                             #  ├─► Design.LayerViolation
app    = ["domain", "infra"]                   #  │   (allowed import directions)
                                                # ─┘
[public_api]                                    # ─┐
exports = ["package.json:exports", "src/index.ts"] ├─► exempts Design.OrphanExport
                                                # ─┘   (entry points aren't "dead")

[boundaries]                                   # ─┐
"src/legacy/**" = { frozen = true, reason = "see ADR-0007" } ├─► Design.BoundaryFrozen
                                                # ─┘   (per-file glob match, flagged if touched)

[invariants]                                   # ─┐
"no-direct-db-access" = { forbid_imports = ["src/infra/db"], from_layers = ["app"] } ├─► Design.InvariantViolation
                                                # ─┘   (named forbid/require import rules)
```

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

### `[invariants.scripted]`

When the `forbid_imports` / `require_imports` shape can't express a
rule — file-level constraints, layer-conditional gates, predicates
over both imports AND exports — declare a **scripted** invariant with
the v1 predicate DSL.

```toml
[invariants.scripted."controller-test-pair"]
when    = "file matches 'src/controllers/**/*.ts'"
require = "exists('tests/' + basename(file))"
message = "Every controller needs a test file under tests/"

[invariants.scripted."ui-no-localstorage"]
when    = "file matches 'ui/**'"
forbid  = "imports 'localStorage'"
message = "UI files must not touch localStorage directly"
```

Each rule has four fields:

* `when` (optional) — predicate that gates the rule. The rule only
  fires on files where `when` evaluates true. Omit to apply to every
  file.
* `require` — predicate that MUST hold. The rule emits a finding when
  it evaluates false.
* `forbid` — predicate that MUST NOT hold. The rule emits a finding
  when it evaluates true.
* `message` — literal text surfaced on each finding.

Exactly one of `require` / `forbid` must be set. Setting both or
neither is a config-load error.

**DSL surface (v1).** Boolean glue (`and`, `or`, `not`, parentheses)
over comparisons; string concat with `+`; functions `basename(...)`,
`dirname(...)`, `exists(...)`. Operators:

| Operator | Purpose |
|---|---|
| `matches` | gitignore-style glob match against a file path |
| `==` / `!=` | string equality on `file.path` / `file.layer` |
| `in '<layer>'` | file resolves to the named layer |
| `imports '<spec>'` | direct import edge to a module specifier or path |
| `transitively imports '<spec>'` | transitive closure (direct-only in v1; full closure in cd-9hp.9) |
| `imports as type '<spec>'` / `imports as value '<spec>'` | type-only vs value imports |
| `exports '<name>'` | file exports a named symbol |

Subjects: `file`, `file.path`, `file.layer`. Namespaces `core.*` and
`ts.*` are reserved in the grammar for future-domain expansion; v1
does not yet resolve them. Full grammar — including the EBNF and the
list of error classes — lives in
[docs/dsl-grammar.md](./dsl-grammar.md).

**Fail-fast at config load.** Every DSL string is parsed when
`cofferdam.invariants.toml` is loaded. A malformed predicate is a
fatal error pointing at the offending rule + field; the engine
refuses to start. Bad scripts never reach file 4000 of the run.

**v1 limitations.**

* Findings carry the file path with line/col `1:1`. Per-edge spans
  for `imports` predicates are reserved for a future MINOR bump.
* `message` is the literal string. `{file}` and friends from the
  grammar doc are forward-compat surface — v1 emits the message
  verbatim and reserves interpolation for v2.
* `transitively imports` evaluates direct edges only in v1 (graph
  closure ships with cd-9hp.9).

Read by `Design.ScriptedInvariant`. All scripted rules share one
check id — suppress per-line via `// cofferdam-disable-next-line
Design.ScriptedInvariant` or globally with a severity override on
the check.

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
