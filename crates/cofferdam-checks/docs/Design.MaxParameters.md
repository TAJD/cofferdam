---
id: Design.MaxParameters
category: Design
base_priority: 5
default_severity: Medium
options: [limit]
---

Functions with too many parameters are hard to call correctly: callers can't remember positional order, and adding a sixth parameter breaks every call site. Pass an options object instead.

```ts
// flagged: 7 parameters
function createUser(
  id: string,
  name: string,
  email: string,
  role: Role,
  team: string,
  createdBy: string,
  notify: boolean,
) { /* ... */ }
```

```ts
// fix: collapse into an options object
interface CreateUserInput {
  id: string;
  name: string;
  email: string;
  role: Role;
  team: string;
  createdBy: string;
  notify?: boolean;
}
function createUser(input: CreateUserInput) { /* ... */ }
```

```toml
[checks."Design.MaxParameters"]
limit = 6           # bump to 6 if your codebase has earned it
severity = "medium"
```
