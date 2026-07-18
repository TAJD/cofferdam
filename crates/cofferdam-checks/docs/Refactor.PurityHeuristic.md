---
id: Refactor.PurityHeuristic
category: Refactor
base_priority: -10
default_severity: Low
options: [enabled]
---

An exported function that reads a module-level `let` binding not covered by its own parameter list — a hidden dependency on outside-the-signature state that works against unit-testability. Only flags when the module-level binding is also reassigned somewhere in the file; read-only module state (config objects, loggers, memoization caches that are never reassigned) is not a mutation dependency and is left alone.

```ts
let requestCount = 0;

export function logRequest(name: string) {
  // reads `requestCount`, a module-level `let` that IS reassigned
  // elsewhere — not covered by this function's own parameters.
  console.log(`${name}: request #${requestCount}`);
}

export function bumpRequestCount() {
  requestCount += 1;
}
```

```ts
// fix — pass the dependency explicitly
export function logRequest(name: string, requestCount: number) {
  console.log(`${name}: request #${requestCount}`);
}
```

Not flagged — module state that's declared `let` but never reassigned anywhere in the file (read-only config, a logger instance, a lazily-populated cache) is a legitimate dependency, not hidden mutation coupling:

```ts
let config = { apiUrl: "https://api.example.com" };

export function fetchData() {
  return fetch(config.apiUrl);
}
```

This is a fuzzy, name-based heuristic (not full scope resolution) with a real false-positive risk, so it ships **disabled by default**:

```toml
# cofferdam.toml
[checks."Refactor.PurityHeuristic"]
enabled = true
```
