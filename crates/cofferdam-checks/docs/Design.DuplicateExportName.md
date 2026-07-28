---
id: Design.DuplicateExportName
category: Design
base_priority: 6
default_severity: Medium
options: [exempt_boundary_pairs]
---

The same name is exported from multiple files. Barrel re-exports collide silently and importers can't tell which one they got. The check runs in the engine's `finalize` pass — per-file `run` collects export names into the shared corpus, then `finalize` groups by name and emits one `Issue` per duplicate set with the canonical occurrence as the primary span and the rest as `related: Vec<RelatedSpan>`.

```ts
// src/users.ts
export function format(u: User) { /* ... */ }   // flagged

// src/posts.ts
export function format(p: Post) { /* ... */ }   // also flagged (related)
```

```ts
// fix: namespace, rename, or pick one canonical home
export function formatUser(u: User) { /* ... */ }
export function formatPost(p: Post) { /* ... */ }
```

## Intentional cross-boundary mirrors

Some projects mirror a name across a boundary on purpose — a client/server
contract pair, or a public re-export that deliberately shadows an internal
name. `exempt_boundary_pairs` exempts exactly those pairings and nothing
else. Each entry names one boundary; its sides are path globs separated by
`|`:

```toml
# cofferdam.toml
[checks."Design.DuplicateExportName"]
exempt_boundary_pairs = ["client/**|server/**", "packages/public/**|packages/internal/**"]
```

A duplicate set is exempt only when **every** occurrence matches a
**distinct** side of a single entry. So:

- `client/schema.ts` + `server/schema.ts` → exempt (one per side).
- `client/a.ts` + `client/b.ts` → still flagged; both are on the same side,
  which is an ordinary collision, not a mirror.
- `client/schema.ts` + `server/schema.ts` + `utils/misc.ts` → still flagged;
  the third file is outside the boundary, so the set is not a clean mirror.
- `client/schema.ts` + `utils/misc.ts` → still flagged; only the declared
  pairing is exempt, so the file stays under scrutiny everywhere else.

More than two sides are allowed (`client/**|server/**|shared/**`). Globs are
matched against the project-root-relative path; an unanchored glob also
matches at any depth, so `client/**` covers `packages/app/client/schema.ts`.
Sides should be mutually exclusive — a path matching several sides is
assigned to the first unclaimed one in declaration order.

Fixtures: `examples/duplicate_export_boundary/`.

If you'd rather not enumerate boundaries at all, demote the whole check:

```toml
# cofferdam.toml
[checks."Design.DuplicateExportName"]
severity = "low"   # demote to info-only if your project relies on barrel collisions
```
