# cd-31r — Layer glob resolution

Status: **approved 2026-05-06**, ready to implement.

## Problem

When two `[layers]` globs in `cofferdam.invariants.toml` overlap, the
alphabetically-first layer wins. `!negation` patterns inside a single
layer's glob list are silently ignored. Users cannot express "leaf
layer wins specificity-wise" except by renaming layers to sort first.

Repro on cofferdam 0.2.2:

```toml
[layers]
ui         = ["components/ui/**"]
components = ["components/**", "!components/ui/**"]
```

`cofferdam advise components/ui/button.tsx` reports `Layer: components`,
expected `ui`.

## Decision

Implement **option (1)** from the bead — honor intra-layer `!`
negations — *and* tighten the cross-layer matcher so the most-specific
include pattern wins. Both parts are needed: (1) alone leaves the
ambiguity when two layers' includes both still match.

Option (2) — document the rename-to-sort workaround — is rejected.
Rename-as-priority is a leaky abstraction that bites every new layer
config.

## Design

### Part 1 — gitignore-style negations within a layer

A layer's glob list is treated as gitignore semantics:

- Patterns that do **not** start with `!` are includes.
- Patterns that **do** start with `!` are excludes.
- A file is in the layer iff at least one include matches AND no
  exclude matches.

Implementation in `cofferdam-core::layers::build_matchers`:

- For each layer, partition the glob list into `includes` and `excludes`.
- Build two `globset::GlobSet` per layer: `include_set`, `exclude_set`.
- A file is in the layer when `include_set.is_match(path) && !exclude_set.is_match(path)`.

### Part 2 — replace alphabetical first-match with specificity

When multiple layers still match a file, pick the layer whose
**most-specific include pattern** has the longest non-glob prefix.
Tie-break: alphabetical (preserves current behavior for true ties).

Specificity metric — for each include pattern, compute the longest
prefix that contains no glob meta-characters (`*`, `?`, `[`, `{`).

Examples:
- `components/ui/**` → prefix `components/ui/` (length 14)
- `components/**` → prefix `components/` (length 11)
- `**/*.test.ts` → prefix `` (length 0)

When a file matches multiple layers, the layer whose matching
include has the longest prefix wins.

Implementation:
- Per layer, store `(include_set, exclude_set, max_prefix_len)`. The
  `max_prefix_len` is computed once at config-load time.
- `layer_for(path)` collects all matching layers, picks max by
  `max_prefix_len`, breaks ties alphabetically.

Worth noting: `max_prefix_len` is computed across **all** include
patterns of a layer, not per-match. Cheaper, and matches user intent
("the layer with the most-specific declaration about this directory").
If a layer has both `src/**` and `src/api/**`, the layer's specificity
is 8 (the api prefix), used for any file the layer claims.

### Tests

Unit tests in `cofferdam-core/src/layers.rs`:

1. Bead's exact repro: `ui` vs `components` with negation — file under
   `components/ui/` resolves to `ui`.
2. Two layers, no overlap (regression).
3. Three-way overlap with three different specificities — most
   specific wins.
4. True tie (two layers with identical specificity) — alphabetical.
5. Negation alone (no second layer): a `!components/ui/**` exclude on
   the only layer makes the file unmatched (`None`).

Integration test: a `cofferdam advise` invocation in a tempdir with
the bead's config asserting `Layer: ui` for `components/ui/button.tsx`
(this is where the user-facing regression would surface first).

### Docs

`docs/checks/Design.LayerViolation.md` (source:
`crates/cofferdam-checks/docs/Design.LayerViolation.md`) — replace:

> The first matching layer (in alphabetical layer-name order) wins,
> so place specific layer globs ahead of broad ones if they overlap.

With:

> When multiple layers match a file, the one with the most-specific
> glob (longest non-glob prefix in its include patterns) wins;
> alphabetical layer name breaks true ties. Use `!pattern` within a
> layer's glob list to carve out subtrees explicitly:
>
> ```toml
> [layers]
> ui         = ["components/ui/**"]
> components = ["components/**", "!components/ui/**"]
> ```

`CHANGELOG.md` — note under fixes: "Layer resolution now picks the
most-specific layer when multiple layer globs match (cd-31r / gh #5).
Honors `!negation` patterns within a single layer's glob list. Configs
that previously relied on alphabetical layer-name ordering for
overlapping globs may see different layer assignments — use a `!`
exclude or rely on prefix specificity instead."

## What this is NOT

- Not a generalisation to gitignore-style precedence across the whole
  `[layers]` section. Layers are categories, not a precedence list.
- Not a config-file-order semantics. TOML doesn't guarantee key order
  on read in all toolchains; relying on file order would be fragile.
- Not a SemVer-major change. Existing configs that worked correctly
  continue to work; only the case where alphabetical ordering was
  *masking* a more-specific layer changes — and that case is the bug.

## Files in scope

- `crates/cofferdam-core/src/layers.rs` — matcher refactor + tests
- `crates/cofferdam-checks/docs/Design.LayerViolation.md` — doc text
- `CHANGELOG.md` — entry under unreleased
- (Controller will run `cofferdam gen-docs` at merge time; do NOT run
  it from the implementation agent.)

## Out of scope

- Multi-layer membership ("file belongs to ui *and* components"). The
  data model supports a single layer per file; adding multi-membership
  is a separate bead.
- Glob spec extensions (e.g. `@(...)`, custom char classes). Stay on
  whatever globset already supports.
