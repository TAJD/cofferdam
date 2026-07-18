---
id: Consistency.ErrorHandlingIdiom
category: Consistency
base_priority: -5
default_severity: Info
options: []
---

The project predominantly uses one error-handling idiom — throwing, or returning an error-shaped value (`{ error: ... }`, `{ ok: false }`, `{ success: false }`) — and this file deviates from it.

```ts
// most of the project throws for errors...
export function parseConfig(raw: string) {
  if (!raw) {
    throw new Error("config is empty");
  }
  return JSON.parse(raw);
}
```

```ts
// ...but this file returns an error-shaped value instead — flagged
export function parseUser(raw: string) {
  if (!raw) {
    return { error: "user payload is empty" };
  }
  return { error: null, value: JSON.parse(raw) };
}
```

```ts
// fix — throw, matching the project's dominant idiom
export function parseUser(raw: string) {
  if (!raw) {
    throw new Error("user payload is empty");
  }
  return JSON.parse(raw);
}
```

Not flagged — the success arm of a Result-shaped return (`{ error: null }`, `{ ok: true }`, `{ success: true }`) isn't a competing error idiom, so it's excluded from the tally either way.

Two-pass, project-wide: pass 1 counts every `throw` statement and every error-shaped `return` across the whole project; pass 2 requires a strict majority (over 50%, and not a tie) before it considers either idiom "dominant" — a 50/50 split, or fewer than 4 total occurrences project-wide, emits nothing. Every occurrence of the minority idiom is then flagged, in whichever file it appears.

Scope: v1 deliberately does not try to group occurrences by "the same kind of failure" (e.g. by directory, domain, or error class/name) — this is a whole-project idiom-consistency signal, not a same-failure-class comparison. A project that legitimately mixes idioms across unrelated domains (e.g. throwing for programmer errors, returning a `Result`-shaped value for expected validation failures) will see this as noise. It overlaps somewhat with `Refactor.MixedThrowAndReturnError`, which flags a single function mixing both idioms internally — this check instead flags idiom inconsistency across the whole project.

Only `throw` statements and explicit `return` statements are counted — a concise arrow body (`const f = () => ({ error: "x" })`) has no `ReturnStatement` node at all, so it isn't tallied (the same blind spot `Refactor.MixedThrowAndReturnError` has), and `Promise.reject(...)` is likewise invisible to this check. A `catch (e) { throw e; }` re-throw counts toward the throw tally the same as any other throw, even though it's propagation rather than a deliberate idiom choice.
