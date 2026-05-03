---
id: Design.DuplicateExportName
category: Design
base_priority: 6
default_severity: Medium
options: []
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

```toml
# cofferdam.toml
[checks."Design.DuplicateExportName"]
severity = "low"   # demote to info-only if your project relies on barrel collisions
```
