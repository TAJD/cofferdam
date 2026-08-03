# Authoring `.cofferdam/knowledge/*.md` notes

Curated knowledge notes are how you tell an AI agent (or a teammate
running [`cofferdam context`](./reference/context.md)) something the
code can't say for itself — the "why," a past incident, a convention
that isn't enforced by a check. They're matched against the current
`ChangeSet` and surfaced through the `Context.Knowledge` provider.

## Where notes live

```
.cofferdam/knowledge/*.md
```

Any `.md` file directly under that directory is loaded. Missing or
unreadable directory → zero notes, no error (a project with no curated
knowledge is the zero-config default).

## File format

Each note is a Markdown file with a YAML frontmatter block followed by
a body:

```markdown
---
title: Order totals are computed in cents, not dollars
priority: high
match:
  paths:
    - "src/domain/order/**/*.ts"
  layers:
    - domain
  predicate: "file imports 'stripe'"
---

`Order.total` and every derived total field is an integer number of
cents. This was a deliberate choice after a rounding bug shipped to
production in 2025 (see PROJ-118) — never introduce a `number` field
here that represents a fractional-dollar amount.
```

### Frontmatter fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `title` | string | Yes | One-line heading shown in the digest. Note load fails entirely (not just the selector) if `title` is missing. |
| `priority` | `high` \| `normal` \| `low` | No (default `normal`) | See [Priority](#priority) below. An unrecognized value warns and falls back to `normal` rather than failing the note. |
| `match.paths` | string[] | No | Glob patterns (via [`globset`](https://docs.rs/globset)), matched against the changed file's path relative to the project root. |
| `match.layers` | string[] | No | Layer names from your `[layers]` config in `cofferdam.invariants.toml`. A changed file matches if it belongs to any listed layer. |
| `match.predicate` | string | No | A predicate-DSL expression — the same language used in `[invariants.scripted]`. Full grammar: [dsl-grammar.md](./dsl-grammar.md). |

At least one of `match.paths`, `match.layers`, `match.predicate` should
be set — see [Selector validation](#selector-validation) for what
happens when none are, or all fail.

A note matches a changed file when **any** valid selector matches (an
OR across `paths` entries, `layers` entries, and `predicate` — not an
AND across the three groups).

## Priority

`priority` controls both ranking and whether the digest's token budget
can evict the note:

| Priority | Digest score | Evictable by `--budget`? |
|---|---|---|
| `high` | 100 | No — pinned, always included, even if it pushes `spent` over `budget`. |
| `normal` (default) | 50 | Yes |
| `low` | 10 | Yes |

Reach for `high` only for the notes where silence is actually costly —
an incident that will recur, a non-obvious invariant a naive edit will
break. Overusing `high` defeats the budget: a project with many `high`
notes on a large change can blow past `--budget` entirely (the digest
discloses this — see `spent > budget` in
[the context reference](./reference/context.md#json-output-for-agents) —
but it's still better avoided than relied on).

## Selector validation

Cofferdam applies the same "warn loudly, never silently match nothing"
policy here as it does for `[invariants.scripted]` rules:

- An invalid glob in `match.paths`, an undeclared layer name in
  `match.layers`, or an unparseable `match.predicate` drops **only
  that selector** (as an `Issue` warning) — the rest of the note's
  selectors still apply.
- A note left with **zero** valid selectors after validation warns
  that it will never fire, since that would otherwise be a silent
  no-op.
- These warnings surface as ordinary `cofferdam context` stderr
  warnings during a normal run, not just under `--lint-knowledge`.

Run `cofferdam context --lint-knowledge` to validate every note
up front — every selector must parse, and every `match.paths`/
`match.layers` selector must match at least one file in the current
repo (catches an orphaned selector: a glob that used to match before a
rename, a layer that got renamed). This is the one command in the
`context` family that exits nonzero on failure, so it's the one to
wire into CI:

```bash
cofferdam context --lint-knowledge
```

## Body length

Note bodies are capped at 8,000 characters. A longer body is truncated
with a visible `[truncated, ...]` marker and a load-time warning — the
cap exists because a single `priority: high` note is pinned and can
otherwise blow any `--budget` on its own. Keep notes focused; link out
to a wiki page or design doc for anything longer than a few paragraphs.

## Example: a layer-scoped note

```markdown
---
title: Never import from infra/ directly in domain/
priority: normal
match:
  layers:
    - domain
---

This is already enforced by `Design.LayerViolation`, but if you're
here because that check fired: the intended pattern is a port/adapter
interface defined in `domain/` and implemented in `infra/`, injected
at the composition root — not a direct import.
```

## Example: predicate-only note

```markdown
---
title: Any file importing the legacy payments client should link PROJ-204
priority: low
match:
  predicate: "file imports 'src/infra/legacy-payments-client'"
---

`legacy-payments-client` is mid-migration to the new Stripe adapter
(PROJ-204). If you're touching a caller, check whether it can move to
`src/infra/payments-client` instead of extending the legacy path.
```

See [dsl-grammar.md](./dsl-grammar.md) for the full predicate language
(`matches`, `imports`, `transitively imports`, `in`, boolean
combinators).

## Related

- [`cofferdam context`](./reference/context.md) — the command that
  surfaces these notes.
- [`cofferdam.invariants.toml`](./invariants.md) — where `[layers]` is
  declared (needed for `match.layers` to validate).
- [Predicate DSL grammar](./dsl-grammar.md) — full `match.predicate`
  syntax.
