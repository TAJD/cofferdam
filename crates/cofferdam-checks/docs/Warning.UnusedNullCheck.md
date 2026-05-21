---
id: Warning.UnusedNullCheck
category: Warning
base_priority: 15
default_severity: Low
options: []
---

An equality comparison against `null` or `undefined` where the other operand's resolved TypeScript type already excludes the value being checked for. The guard can never change the outcome — it's either dead defensive code, or a signal that the type annotation disagrees with reality.

This is cofferdam's first **type-aware** check (`requires_types: true`). It's routed through the ts-morph type host rather than the pure-Rust pipeline, so it only runs when a type host is available — a project with `tsconfig.json` and `ts-morph` installed, and `[engine] type_aware` not disabled. When no host is available the engine skips the check entirely (no false positives from missing type info).

The comparison semantics mirror JavaScript's nullish equality exactly:

- `x == null` / `x != null` (**loose**) match *both* `null` and `undefined`. Redundant only when the type excludes both.
- `x === null` / `x !== null` (**strict**) test `null` alone.
- `x === undefined` / `x !== undefined` test `undefined` alone.

The check **bails on `any` and `unknown`** — the compiler can't prove a guard redundant against a type it knows nothing about, so flagging would be a false positive.

```ts
// flagged — `s` is `string`, which excludes null and undefined
function f(s: string) {
  if (s !== null) return s;       // redundant
  if (s != null) return s;        // redundant (loose, excludes both)
}
```

```ts
// not flagged — `s` genuinely includes the value being checked
function g(s: string | null) {
  if (s !== null) return s;       // meaningful: s can be null
}
function h(s: string | undefined) {
  if (s === undefined) return "";  // meaningful: s can be undefined
}
```

```ts
// not flagged — strict `!== null` against a type that only adds undefined
function k(s: string | undefined) {
  if (s !== null) return s;       // s can't be null, but === null is the wrong test;
                                  // strict-null is NOT redundant only when null is possible.
}
```

```ts
// not flagged — `any` / `unknown` defeat the analysis
function u(x: any) {
  if (x != null) return x;
}
```

No autofix: removing a guard changes control flow, so the safe action is human review, not a mechanical edit.

```toml
# Raise from the default (low) if you want redundant guards to gate CI.
[checks."Warning.UnusedNullCheck"]
severity = "medium"
```
