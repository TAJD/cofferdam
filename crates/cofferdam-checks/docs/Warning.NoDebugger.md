---
id: Warning.NoDebugger
category: Warning
base_priority: 10
default_severity: Medium
options: []
---

`debugger` statements halt execution under attached devtools. Always a debugging leftover in shipped code — there's no benign use case in production builds. The fix is mechanical: delete the line.

```ts
// flagged
function inspect(x: unknown) {
  debugger;
  return x;
}
```

```ts
// fix
function inspect(x: unknown) {
  return x;
}
```
