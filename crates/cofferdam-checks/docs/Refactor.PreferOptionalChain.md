---
id: Refactor.PreferOptionalChain
category: Refactor
base_priority: 5
default_severity: Low
options: []
---

`a && a.b && a.b.c` is more concisely written as `a?.b?.c`. The optional-chain operator (`?.`) short-circuits on `null`/`undefined`. The check flags `lhs && rhs` where the LHS is a "safe to repeat" expression (identifier, `this`, or a pure member chain — never contains a `CallExpression` or `NewExpression`) and the RHS is a member access (or call on a member access) whose object span renders to the same source bytes as the LHS span.

```ts
// flagged
return user && user.profile;
return user && user.profile && user.profile.name;
return arr && arr[0];
```

```ts
// fix
return user?.profile;
return user?.profile?.name;
return arr?.[0];
```

```ts
// not flagged (LHS is a call — rewriting would halve the call count)
return get() && get().profile;
```
