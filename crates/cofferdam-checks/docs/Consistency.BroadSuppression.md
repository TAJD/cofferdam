---
id: Consistency.BroadSuppression
category: Consistency
base_priority: 0
default_severity: Info
options: []
---

`// cofferdam-ignore` (with no check id) silences every check on the next non-blank line. That makes suppression intent invisible to reviewers — they have to read the surrounding code to guess what was being suppressed.

Two narrowed forms are accepted; both pin a check id and an optional reason:

- `// cofferdam-ignore: <Check.Id>: <reason>` — colon-separator (canonical Biome-style).
- `// cofferdam-ignore <Check.Id> — <reason>` — space-separator with em-dash, ASCII hyphen, or colon between id and reason (cd-b77).

Either form binds the id and silences only the named check. Future readers (and linters) can audit it.

This diagnostic is informational — it never fails CI on its own. To suppress this nudge for a specific intentional broad form, write:

```
// cofferdam-ignore: Consistency.BroadSuppression: chosen broad scope intentionally
// cofferdam-ignore
some_call_that_legitimately_needs_broad_silence();
```
