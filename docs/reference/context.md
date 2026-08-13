---
title: cofferdam context
---

# `cofferdam context` — the default entrypoint for AI agents

`cofferdam context <paths>` answers the question an agent almost always
needs answered first: **"what do I need to know about this change that
I don't already have?"** It resolves the current diff (or explicit
paths) to a `ChangeSet`, runs the full engine over the project, and
prints a token-budgeted digest assembled from five providers: fresh
findings on the lines you touched, files that import what you changed
(blast radius), sibling-file conventions your change should probably
follow (precedent), curated `.cofferdam/knowledge/*.md` notes, and
inline `// @cofferdam-context:` annotations near the code you're
editing.

It is **advisory only** — it never fails the build. Exit code is `0`
except on a usage or git-resolution error (`lint_knowledge` mode is the
one deliberate exception; see below). Run it at the start of a task, or
right after making a change, even when you have no prior conversation
context at all — that's the point of it.

For rule-level output (what's wrong with the code right now), use
[`cofferdam check`](./cli.md#cofferdam-check). For per-file constraints
before you edit, use [`cofferdam advise`](./advise.md). `context` sits
above both: it's the "what's relevant here" digest an agent reads
first.

## Quick start

```bash
# Working-tree diff vs HEAD, markdown digest.
cofferdam context

# Same, but JSON — --robot defaults --format to json.
cofferdam context --robot --pretty
```

```json
{
  "schema_version": 1,
  "changed_files": ["src/domain/order.ts"],
  "items": [
    {
      "check_id": "Context.BlastRadius",
      "title": "3 file(s) import src/domain/order.ts",
      "body": "src/api/checkout.ts, src/api/refund.ts, src/jobs/reconcile.ts",
      "score": 70,
      "pinned": false,
      "explain": "src/domain/order.ts is imported by 3 file(s) in the project graph"
    }
  ],
  "omitted": 0,
  "budget": 2000,
  "spent": 41
}
```

## What agents should branch on

| Field | Where | Branch on |
|---|---|---|
| `items[].check_id` | per item | Which provider emitted it (`Context.Findings`, `Context.BlastRadius`, `Context.Precedent`, `Context.Knowledge`, `Context.Annotations`) — lets an agent weight or filter by source. |
| `items[].pinned` | per item | `true` → this item survived budget truncation deliberately (a real finding, or a `priority: high` knowledge note); never treat a pinned item as low-signal filler. |
| `items[].explain` | per item | Always populated — the concrete reason this item fired (a selector, an import edge, a distance). Read this before the body when deciding whether an item is relevant. |
| `omitted` | top-level | Non-zero → real content was cut for budget; consider rerunning with `--budget` raised before concluding "nothing else matters." |
| `spent > budget` | top-level | Pinned items pushed the digest over budget — expected behaviour, not a bug. |

## Flags

Every flag is listed in [the CLI reference](/reference/cli#cofferdam-context), which is generated from the binary and so cannot drift. The two that change what the command *does* rather than how it prints — `--lint-knowledge` and `--lint-context-suppress` — are covered below.

## Text output

```text
# Cofferdam context — 1 changed file(s)

## 3 file(s) import src/domain/order.ts  `Context.BlastRadius`

src/api/checkout.ts, src/api/refund.ts, src/jobs/reconcile.ts

_why: src/domain/order.ts is imported by 3 file(s) in the project graph_

## Sibling files in this directory use `Result<T, DomainError>`  `Context.Precedent`

3 of 4 sibling files return `Result<T, DomainError>` from exported
functions; this file's new export returns a bare `T`.

_why: src/domain/*.ts precedent, sampled from 4 sibling file(s)_
```

An empty digest for a real diff is honest, not silent:

```text
No relevant context found for 1 changed file(s).
```

When items are truncated by budget, the digest says so explicitly
rather than silently dropping them:

```text
_2 item(s) omitted (budget 2000 tokens); rerun with a larger --budget._
```

## JSON output (for agents)

`cofferdam context --robot --pretty` produces a schema-versioned
envelope: `{schema_version, changed_files, items, omitted, budget,
spent}`. Stable keys within a `schema_version`; additive changes only.

### Field reference

Envelope:

| Key | Type | Notes |
|---|---|---|
| `schema_version` | integer | `1` today. |
| `changed_files` | string[] | The resolved `ChangeSet`'s file paths, relative to the project root and forward-slashed (CD-241) — path-bearing fields on `items` (`title`, `body`, `explain`, `related[].file`) are relativized the same way. Capped at 500 entries (CD-265); see `changed_files_truncated_from`. |
| `changed_files_truncated_from` | integer | Present only when `changed_files` was capped — the true (pre-cap) file count. Omitted entirely when nothing was cut. |
| `items` | array | The digest, after budget truncation — see `ContextItem` below. |
| `omitted` | integer | Count of items that scored/ranked below the cutoff and were dropped for budget. `0` means nothing was cut. |
| `budget` | integer | The `--budget` value used. |
| `spent` | integer | Actual tokens spent on `items` (each item's field content plus a fixed per-item rendering overhead, CD-246). Can exceed `budget` when pinned items push the total over — pinned items are never evicted. Does **not** include `changed_files` — CD-265 bounds `changed_files` to 500 entries so it can no longer dwarf `spent`, but its own token cost still isn't charged against `budget`. |

Per `ContextItem`:

| Key | Type | Notes |
|---|---|---|
| `check_id` | string | The emitting provider's id: `Context.Findings`, `Context.BlastRadius`, `Context.Precedent`, `Context.Knowledge`, or `Context.Annotations`. |
| `title` | string | One-line heading. |
| `body` | string | Markdown body — the actual content. |
| `score` | integer | Relevance; higher sorts earlier. Provider-relative, not comparable across a redesign of any one provider. |
| `pinned` | bool | `true` → never evicted by budget truncation (real findings; `priority: high` knowledge notes). |
| `related` | array \| omitted | Present when the item points at specific spans beyond the changed file(s) — `{file, location}` per entry. Omitted when empty. |
| `explain` | string \| omitted | Why this item fired — a selector, an import edge, a distance. Always populated by every built-in provider today; the key is optional in the schema for forward-compatibility with a provider that can't produce one. |

## The five providers

| `check_id` | Fires on | What it tells you |
|---|---|---|
| `Context.Findings` | Findings on lines your `ChangeSet` touches | A summary of what `cofferdam check` would already flag on the changed lines — so you don't have to run `check` separately just to see if your edit is already non-compliant. |
| `Context.BlastRadius` | Files that import a file in your `ChangeSet` | Who else in the project depends on what you're changing — the set of files a behaviour change here could silently break. |
| `Context.Precedent` | Sibling files in the same directory as a changed file | Conventions your change should probably follow (a return-type pattern, a naming scheme) inferred from files next to it, not from a style guide. |
| `Context.Knowledge` | `.cofferdam/knowledge/*.md` notes whose selectors match a changed file | Curated, human-written context — the "why," not derivable from the code alone. See [authoring knowledge notes](../knowledge-notes.md). |
| `Context.Annotations` | `// @cofferdam-context: ...` comments in or near a changed file's enclosing function/class | Inline notes left directly in the code, scoped to the enclosing declaration (or the whole file, for a top-level annotation) — and surfaced too for files that import an annotated scope. |

## `--lint-knowledge` mode

`cofferdam context --lint-knowledge` validates every
`.cofferdam/knowledge/*.md` note instead of producing a digest: every
selector must parse, and every `match.paths`/`match.layers` selector
must match at least one file in the current repo (catches a broken
glob or a typo'd layer name before it silently never fires). This is
the one deliberate nonzero-exit carve-out in `cofferdam context` — it
exits nonzero when validation fails, so CI can gate a PR that adds a
broken note.

```bash
cofferdam context --lint-knowledge
```

## `[[context_suppress]]` — suppressing noisy digest items

A `[[context_suppress]]` block in `cofferdam.toml` (CD-212) drops
matching items from the digest before it's assembled, for the case
where a provider is technically correct but not useful for this
project — a `Context.Precedent` convention that's actually deliberate
divergence, a `Context.Knowledge` note that's gone stale but hasn't
been deleted yet.

```toml
[[context_suppress]]
check_id = "Context.Precedent"
paths = ["src/legacy/**"]
reason = "src/legacy intentionally predates the current convention"

[[context_suppress]]
check_id = "Context.Findings"
```

| Key | Type | Effect |
|---|---|---|
| `check_id` | string, required | The provider id to suppress items from — `Context.Findings`, `Context.BlastRadius`, `Context.Precedent`, `Context.Knowledge`, or `Context.Annotations`. |
| `paths` | string[], optional | Glob(s), matched the same way as [`[[overrides]]`](/overrides) — project-root-relative, forward-slash, `**` crosses directory separators. An item is suppressed when *any* of its `related` spans' files match *any* glob here. |
| `reason` | string, optional | Free text, surfaced in `--lint-context-suppress` diagnostics; has no effect on matching. |

**Omitting `paths` suppresses every item the `check_id` emits, related
or not** (CD-227) — this is the *only* way to suppress an item that
has no `related` span at all, such as `Context.Findings`'s
"N pre-existing finding(s) outside the diff" summary or
`Context.Precedent`'s "matching skipped for N oversized group(s)"
advisory (CD-228/CD-235). Both are relatedless by design (they don't
point at one specific file), so a `paths`-scoped rule can never match
them — the second example in the block above turns off
`Context.Findings` entirely, including that summary item.

**This is the opposite of `[[overrides]]`'s convention for the same
shape.** An `[[overrides]]` block with `paths` omitted compiles to an
empty globset and matches **nothing** — the block does nothing. A
`[[context_suppress]]` block with `paths` omitted matches
**everything** the `check_id` emits. Same TOML shape (`paths` absent),
opposite meaning — don't carry an intuition from one config surface to
the other.

### `--lint-context-suppress` mode

`cofferdam context --lint-context-suppress` validates every
`[[context_suppress]]` rule instead of producing a digest:

- **Unknown `check_id`** (a typo, e.g. `Context.Percedent`) always
  fails, for both path-scoped and wildcard rules (CD-233) — validated
  against the real provider set, no hardcoded id list.
- **Stale `paths` glob** — a path-scoped rule whose globs match zero
  files in the current repo almost certainly targeted files that have
  since moved, been renamed, or been deleted. A wildcard rule (`paths`
  omitted) is exempt from this specific check, since "matches 0 files"
  is meaningless for a rule that isn't matching by path in the first
  place.

Same nonzero-exit carve-out as `--lint-knowledge`.

```bash
cofferdam context --lint-context-suppress
```

## Why a separate command (vs. `check` / `advise`)

`check` and `advise` both operate on a single file's rules. `context`
operates on a *change* and answers a broader question — not "what
rules apply" or "what's wrong," but "what do I need to know that I
don't already have." It's designed to be the first command an agent
runs on a task with no prior context, and the first command it runs
again right after making an edit.

## Limitations

- `budget` is a crude 4-chars-per-token estimate, not a real tokenizer
  — treat it as a rough sizing knob, not an exact model-token count.
- Discovery reads the whole project to build the cross-file graph
  (blast radius, precedent), so the first run on a cold cache is not
  as cheap as `advise`'s single-file mode.
- The schema is additive — fields may be added in minor versions, but
  existing keys keep their meaning.
