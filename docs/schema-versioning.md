# Schema versioning policy

Cofferdam exposes three schemas to projects and plugins. All three will
evolve. Without an explicit versioning discipline, every engine release
risks silently re-interpreting a long-lived spec or rule file in a way
the project did not consent to. This document is the contract that
prevents that.

## The three schemas

| Schema | Surface | Owner |
|---|---|---|
| `cofferdam.invariants.toml` | The project architecture spec — layers, boundaries, invariants, public API | cd-9hp.12 (this doc) |
| Canonical graph | Emitted by adapters, consumed by rules | cd-T1 / cd-9hp.9 |
| Predicate DSL | Surface syntax for scripted rules inside `cofferdam.invariants.toml` | cd-9hp.1 |

Each is versioned independently. They share the same policy, defined
below.

## Version format

`MAJOR.MINOR`. Semver-flavoured but only two components — there is no
PATCH because schemas don't carry implementation bugs.

Accepted on the wire as either:

* an integer (`schema_version = 1`) — treated as `MAJOR.0`
* a string (`schema_version = "1.0"`, `schema_version = "1.2"`)

The canonical form on serialisation (e.g. via `cofferdam advise` or any
future `cofferdam invariants normalize`) is the string form.

## When to bump

* **MINOR** — additive, backward-compatible changes. New optional
  fields. New node/edge types in the canonical graph. New DSL
  predicates. A spec written for an older MINOR continues to load
  unchanged.
* **MAJOR** — breaking changes. Removed fields. Changed semantics for
  an existing field. Renamed predicates. Default-value changes that
  would silently re-interpret existing specs.

A MAJOR bump must also:

1. Extend `MIN_SUPPORTED_SCHEMA_VERSION` only after the previous MAJOR
   has been deprecated for at least 2 MAJOR releases (see below).
2. Ship a documented migration recipe — manual instructions at minimum,
   `cofferdam invariants migrate` if the change can be mechanically
   applied.
3. Add a CHANGELOG entry under `### Schema changes` describing the
   break and pointing to the migration recipe.

## Deprecation window

A version `v` is **supported** when `MIN_SUPPORTED ≤ v ≤ CURRENT`.

Within that window:

* `v == CURRENT` — no action.
* `MIN_SUPPORTED ≤ v < CURRENT` — accepted, with a one-time deprecation
  hint emitted by the engine. The user's rules apply unchanged.

Outside the window:

* `v > CURRENT` — rejected with: *"schema_version X.Y exceeds this
  build's maximum supported version (CURRENT); upgrade cofferdam or pin
  the spec to a version your build understands."*
* `v < MIN_SUPPORTED` — rejected with: *"schema_version X.Y is no
  longer supported by this build (minimum supported is MIN_SUPPORTED);
  run `cofferdam invariants migrate` against an older cofferdam release
  or update the spec to a supported version."*

Recommended window sizes:

* **MINOR** bumps — no deprecation (additive changes are
  backward-compatible by construction). All MINORs of the current MAJOR
  are supported.
* **MAJOR** bumps — keep the previous MAJOR in the support window for
  at least 2 MAJOR releases past it. That is, when `vN+1.0` ships, `vN`
  stays accepted (with a hint) until `vN+3.0` ships, at which point
  `vN` falls out of the window and `MIN_SUPPORTED` becomes `vN+1.0`.

The window can be longer, never shorter. Tightening below 2 MAJORs
breaks the contract.

## Missing fields

A spec without an explicit `schema_version` field is treated as
`CURRENT_SCHEMA_VERSION` for backwards compatibility with the v0
surface that predates this policy. The engine emits a one-time hint
encouraging the user to declare the field explicitly. From the first
MAJOR bump onward, projects should treat the explicit declaration as
required hygiene — relying on the implicit default leaves them exposed
to a silent re-interpretation at the next MAJOR.

## Today's policy

```rust
pub const CURRENT_SCHEMA_VERSION       = SchemaVersion { major: 1, minor: 0 };
pub const MIN_SUPPORTED_SCHEMA_VERSION = SchemaVersion { major: 1, minor: 0 };
```

For `cofferdam.invariants.toml` today: only `1.0` is accepted; any
other declared value is rejected. The canonical graph and predicate DSL
inherit the same policy once they ship (cd-T1, cd-9hp.1).

## What is versioned and what isn't

**Versioned:**

* The TOML field set on `cofferdam.invariants.toml`.
* The shape of nodes and edges in the canonical graph.
* The surface syntax of the predicate DSL.

**Not versioned (intentionally):**

* `cofferdam.toml` per-check options. Each option's stability is
  governed by the owning check's `CheckMeta`; the option layer never
  re-interprets a long-lived value.
* The `Issue` / `Finding` JSON output schema. This is additive-only and
  documented under `docs/public/checks.json`'s own `schema_version`
  field — see the gen-docs pipeline.
* Internal Rust APIs. Not a user-facing surface.

## Implementation reference

The reference implementation lives in
`crates/cofferdam-core/src/invariants.rs`:

* `SchemaVersion` — the `MAJOR.MINOR` tuple, with `parse_str` and
  `Display`.
* `CURRENT_SCHEMA_VERSION`, `MIN_SUPPORTED_SCHEMA_VERSION` — constants
  bumped on each release.
* `validate_version(declared, current, min_supported) -> VersionCheck`
  — pure function exercised across the full matrix in unit tests so the
  deprecation-window logic is testable today, before MAJOR=2 of any
  schema exists.
* `InvariantsError::{FutureSchemaVersion, UnsupportedSchemaVersion,
  MalformedSchemaVersion}` — actionable error variants with
  `is_fatal()` returning `true`, so the engine fails loudly rather than
  silently ignoring the spec.

The canonical-graph and DSL halves of this policy ship with their own
beads (cd-T1 / cd-9hp.9 for the graph; cd-9hp.1 for the DSL). Both
should reuse `SchemaVersion`, `validate_version`, and the same
deprecation policy described here — same trio of error variants, same
loudness contract.

## Release process

When a schema-touching change lands:

1. If MINOR — bump `CURRENT_SCHEMA_VERSION.minor`.
2. If MAJOR — bump `CURRENT_SCHEMA_VERSION.major`, reset `.minor = 0`,
   write the migration recipe, and (only if the previous MAJOR has
   been deprecated for at least 2 MAJORs) bump
   `MIN_SUPPORTED_SCHEMA_VERSION`.
3. Update every fixture in `crates/cofferdam-engine/tests/spec_contract/`
   to declare the new version.
4. Add a `### Schema changes` block to the CHANGELOG entry for the
   release; link the migration recipe.

A CI gate that fails the release when a schema-touching commit
forgets to update the relevant version constant is tracked separately
(see the TODO at the bottom of this file).

## TODO

* CI gate: fail the release workflow if any of (a) the TomlDoc struct,
  (b) the canonical-graph schema module, (c) the DSL parser change
  without a matching version constant bump and CHANGELOG entry. Tracked
  alongside cd-9hp.12; not yet implemented.
* `cofferdam invariants migrate <input>` — one-shot migration tool.
  Stub until the first MAJOR bump makes it earn its keep.
