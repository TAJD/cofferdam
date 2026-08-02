# Context.Knowledge

Advisory `cofferdam context` provider (CD-162). Loads curated notes
from `.cofferdam/knowledge/*.md` and surfaces any note whose selector
matches at least one file in the current change.

Not part of `cofferdam check` — this check's category is `Context`,
so it is registered only via `all_context_providers()` and only runs
under `cofferdam context`.

## Authoring a note

One markdown file per note under `.cofferdam/knowledge/`, YAML
frontmatter binding it to the change:

```markdown
---
title: Billing invariants
match:
  paths: ["src/billing/**"]           # glob selector
  layers: ["billing"]                 # layer selector (from cofferdam.invariants.toml)
  predicate: "imports 'src/db'"       # optional, reuses the predicate DSL
priority: high                        # high | normal | low
---
Billing code must never round intermediate values; all money math goes
through `Money` in src/billing/money.ts.
```

* `title` — required, one-line heading shown in the digest.
* `match.paths` — gitignore-style glob patterns, matched against the
  project-relative path of each changed file.
* `match.layers` — layer names from `cofferdam.invariants.toml`'s
  `[layers]` table.
* `match.predicate` — an optional predicate-DSL expression (see
  `Design.ScriptedInvariant`'s docs for the grammar), evaluated per
  changed file.
* `priority` — `high` notes are pinned (never evicted by the digest's
  budget truncation); `normal`/`low` set the ranking score.

A note fires when **any** changed file matches **any** selector.

## Validation

Selectors are validated at load: an invalid glob or predicate drops
just that selector and emits a warning (never silently "matches
nothing"). A note left with zero valid selectors after validation
warns that it will never fire. Run `cofferdam context --lint-knowledge`
to validate every knowledge file against the repo and catch broken
globs / orphan selectors in CI.
