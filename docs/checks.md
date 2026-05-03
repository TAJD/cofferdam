# Built-in check catalog

Need to suppress a finding? See [suppression.md](suppression.md).

> To scope cofferdam at the file level, see [`docs/ignore.md`](ignore.md).

Reference for every check shipped in the `cofferdam` binary. Each entry lists the dotted ID, default priority and severity, what it flags and why, a bad/good example, configurable options, and the suppression directive shape.

To inspect the same metadata from the CLI:

```sh
cofferdam explain Warning.TripleEquals
cofferdam explain Refactor.CyclomaticComplexity --robot
```

The `--robot` form emits the JSON shape `{ id, category, default_severity, base_priority, requires_types, consistency, options, explanation }`.

## Index

By category:

- **[Consistency](#consistency)** — `Consistency.QuoteStyle` *(planned)*
- **[Design](#design)** — `Design.DuplicateExportName`, `Design.MaxParameters`
- **[Readability](#readability)** — `Readability.MaxFunctionLength`, `Readability.MaxLineLength`
- **[Refactor](#refactor)** — `Refactor.CognitiveComplexity`, `Refactor.CyclomaticComplexity`, `Refactor.DuplicateBlock`, `Refactor.PreferNullishCoalescing`, `Refactor.PreferOptionalChain`
- **[Warning](#warning)** — `Warning.NoConsoleLog`, `Warning.NoDebugger`, `Warning.NoEval`, `Warning.ParseError`, `Warning.TripleEquals`

## How to read this catalog

- **Priority** is *computed* and sorts the report — higher fixes first. Range `-20..=20`. Users do not configure it.
- **Severity** is *configured* and decides what fails CI. Override per-check in `cofferdam.toml` via `[checks."X.Y"] severity = "..."`. Levels: `info`, `low`, `medium`, `high`, `critical`. The default `--fail-on=medium` fails CI on `medium` or higher. Baselined findings never trigger the gate.
- **Options** are validated against each check's schema at startup. Unknown keys and type mismatches fail loudly. Missing keys fall back to the documented default.
- **Suppression** uses inline directives (line or block scope). The shapes are universal — see [Suppression directives](#suppression-directives) at the bottom.

## Consistency

### `Consistency.QuoteStyle` *(planned)*

| Field | Value |
|---|---|
| Priority | `0` |
| Severity | `low` |
| Two-pass | yes |
| Type-aware | no |
| Options | _none_ |

Mixed quote styles within a project hurt scannability. The full implementation is gated on the engine's two-pass mode (cd-d1y): pass 1 learns the dominant quote style across the corpus; pass 2 flags deviations. Today this is a stub that emits no findings.

## Design

### `Design.DuplicateExportName`

| Field | Value |
|---|---|
| Priority | `6` |
| Severity | `medium` |
| Cross-file | yes |
| Type-aware | no |
| Options | _none_ |

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

### `Design.MaxParameters`

| Field | Value |
|---|---|
| Priority | `5` |
| Severity | `medium` |
| Type-aware | no |
| Options | `limit: int` (default `5`) |

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

## Readability

### `Readability.MaxFunctionLength`

| Field | Value |
|---|---|
| Priority | `-5` |
| Severity | `low` |
| Type-aware | no |
| Options | `limit: int` (default `50`) |

Functions longer than the configured limit are hard to follow. Break them into smaller helpers. The metric counts lines in the function body, not the whole declaration.

```ts
// flagged: body > 50 lines
function processOrder(order: Order) {
  // ... 80 lines of validation, pricing, side effects, and persistence
}
```

```ts
// fix: extract pure helpers with single responsibilities
function processOrder(order: Order) {
  const validated = validateOrder(order);
  const priced = priceOrder(validated);
  return persistOrder(priced);
}
```

```toml
[checks."Readability.MaxFunctionLength"]
limit = 80
severity = "low"
```

### `Readability.MaxLineLength`

| Field | Value |
|---|---|
| Priority | `-5` |
| Severity | `low` |
| Type-aware | no |
| Options | `limit: int` (default `120`) |

Lines longer than the configured limit are harder to scan and review. The check is a pure text-line scan — no AST traversal, so it runs even when a file fails to parse.

```ts
// flagged: > 120 columns
const config = { name: "very long literal", flags: ["a", "b", "c", "d", "e", "f"], description: "..." };
```

```ts
// fix: break across lines
const config = {
  name: "very long literal",
  flags: ["a", "b", "c", "d", "e", "f"],
  description: "...",
};
```

```toml
[checks."Readability.MaxLineLength"]
limit = 100
severity = "info"
```

## Refactor

### `Refactor.CognitiveComplexity`

| Field | Value |
|---|---|
| Priority | `10` |
| Severity | `medium` |
| Type-aware | no |
| Options | _none_ (limit hardcoded at `15`) |

Sonar-style cognitive complexity per function — branching breaks plus a nesting penalty. Deeply nested code costs more than a long flat switch. Tracks `if`/`else if`, loops, ternaries, `switch`, `catch`, sequences of `&&`/`||`/`??`, and recursion-by-name. Default limit is `15`.

```ts
// flagged: nested branches stack a nesting penalty
function classify(record: Record) {
  if (record.kind === "user") {
    if (record.active) {
      for (const role of record.roles) {
        if (role.permissions.includes("admin")) {
          return "active-admin";
        }
      }
    }
  }
  return "other";
}
```

```ts
// fix: flatten via early returns and helpers
function classify(record: Record) {
  if (record.kind !== "user" || !record.active) return "other";
  return hasAdmin(record.roles) ? "active-admin" : "other";
}
```

The hardcoded limit will move into `options` once the threshold becomes contentious in practice (no current bead).

### `Refactor.CyclomaticComplexity`

| Field | Value |
|---|---|
| Priority | `8` |
| Severity | `medium` |
| Type-aware | no |
| Options | _none_ (limit hardcoded at `10`) |

McCabe cyclomatic complexity per function — independent paths through the body. Starts at `1` and adds `1` for each branching node: `if`, each non-default `case`, each loop, ternary, `catch`, and each `&&` / `||` / `??` in conditions. Plain `else` does not add a path. Default limit is `10`.

Cyclomatic and cognitive complexity often flag the same functions but rank them differently. Both are useful — cyclomatic captures "how many test cases do I need", cognitive captures "how hard is this to read". Run them together; the worst offenders fail both.

```ts
// flagged: 11 independent paths via &&/case/if combinations
function dispatch(event: Event) {
  switch (event.kind) {
    case "create": return event.payload && event.payload.id ? handleCreate(event) : null;
    case "update": return event.payload && event.payload.id ? handleUpdate(event) : null;
    case "delete": return event.payload && event.payload.id ? handleDelete(event) : null;
    case "ping": return null;
    default: return null;
  }
}
```

```ts
// fix: dispatch table flattens the case explosion
const handlers = { create: handleCreate, update: handleUpdate, delete: handleDelete } as const;
function dispatch(event: Event) {
  const handler = handlers[event.kind as keyof typeof handlers];
  if (!handler || !event.payload?.id) return null;
  return handler(event);
}
```

### `Refactor.DuplicateBlock`

| Field | Value |
|---|---|
| Priority | `12` |
| Severity | `medium` |
| Cross-file | yes |
| Type-aware | no |
| Options | _none_ (min-statements hardcoded at `6`, min-chars at `80`) |

Runs of statements that recur (after rename canonicalisation) in multiple files. Likely copy-paste — extract a shared helper. Canonicalisation maps identifier tokens to per-window local indices so renamed copies still match. Minimum window is `6` consecutive statements (and `80` characters) to keep noise low. Cross-file: per-file `run` writes fingerprints into the shared corpus; `finalize` groups by hash and emits one `Issue` per duplicate set with `related` spans pointing at every other occurrence.

```ts
// src/orders.ts:42
const items = parseItems(input);
const validated = validateItems(items);
const priced = priceItems(validated, currency);
const taxed = applyTax(priced, region);
const total = sumItems(taxed);
return { items: taxed, total };
```

```ts
// src/quotes.ts:88 — same shape, renamed: flagged as related
const products = parseItems(input);
const checkedProducts = validateItems(products);
const pricedProducts = priceItems(checkedProducts, currency);
const taxedProducts = applyTax(pricedProducts, region);
const total = sumItems(taxedProducts);
return { items: taxedProducts, total };
```

```ts
// fix: extract once
export function pipeline(input: RawInput, currency: Currency, region: Region) {
  const items = parseItems(input);
  const validated = validateItems(items);
  const priced = priceItems(validated, currency);
  const taxed = applyTax(priced, region);
  return { items: taxed, total: sumItems(taxed) };
}
```

### `Refactor.PreferNullishCoalescing`

| Field | Value |
|---|---|
| Priority | `3` |
| Severity | `low` |
| Type-aware | no |
| Options | _none_ |

`x || default` falls through on every falsy value (`0`, `""`, `false`). Use `??` to fall through only on `null`/`undefined`. Today the check ships the narrow high-confidence shape: `member-access || default-literal` (string / number / bool / `null` / bare `undefined` / array literal / object literal). Bare-identifier LHS, function-call LHS, and arithmetic LHS are deliberately not flagged — they're too often genuine alt-branch logic without type info. The rule broadens once the type-aware tier lands.

```ts
// flagged
const timeout = config.timeout || 5000;     // 0 should NOT fall through
const name = user.name || "anonymous";      // "" should NOT fall through
```

```ts
// fix
const timeout = config.timeout ?? 5000;
const name = user.name ?? "anonymous";
```

```ts
// not flagged (intentional falsy fallthrough)
const flag = isAdmin || isOwner;            // bare identifier — alt branch
const value = compute() || 0;               // call result — return type ambiguous
const sum = (a + b) || 1;                   // arithmetic — clearly wants falsy
```

### `Refactor.PreferOptionalChain`

| Field | Value |
|---|---|
| Priority | `5` |
| Severity | `low` |
| Type-aware | no |
| Options | _none_ |

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

## Warning

### `Warning.NoConsoleLog`

| Field | Value |
|---|---|
| Priority | `-10` |
| Severity | `low` |
| Type-aware | no |
| Options | _none_ |

`console.X(...)` calls are typically debugging leftovers. Route logs through a dedicated logger or strip them in CI. The check matches every method on `console` (`log`, `info`, `warn`, `error`, `debug`, `trace`, …) so projects can suppress per-call if they genuinely route errors via `console.error`. Bare-identifier match only — aliased calls (`const c = console; c.log(...)`) escape detection until the type-aware pass.

```ts
// flagged
console.log("debug:", value);
console.error("failed:", err);
```

```ts
// fix: route through a logger that strips in production
import { logger } from "./logger";
logger.debug("debug:", value);
logger.error("failed:", err);
```

```toml
# Demote to info-only if you genuinely use console.error in error paths.
[checks."Warning.NoConsoleLog"]
severity = "info"
```

### `Warning.NoDebugger`

| Field | Value |
|---|---|
| Priority | `10` |
| Severity | `medium` |
| Type-aware | no |
| Options | _none_ |

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

### `Warning.NoEval`

| Field | Value |
|---|---|
| Priority | `18` |
| Severity | `high` |
| Type-aware | no |
| Options | _none_ |

`eval(...)` and `new Function(...)` execute arbitrary strings as code — both are universally banned in security-conscious codebases. The check has no opt-in for a reason. If you have a vetted, isolated use, suppress per-line with an inline directive. Aliased calls (`const f = eval; f("...")`) are out of scope — the AST-only pass matches bare identifiers only.

```ts
// flagged
return eval(userInput);                       // direct eval
return new Function("a", "b", body);          // eval-equivalent
```

```ts
// fix: parse what you actually need, don't execute strings
return JSON.parse(userInput);                 // for JSON
return template(parameters);                  // for templating, use a real engine
```

### `Warning.ParseError`

| Field | Value |
|---|---|
| Priority | `20` |
| Severity | `critical` |
| Type-aware | no |
| Options | _none_ |

Engine-internal: emitted when oxc fails to parse a file. The diagnostic message is included verbatim. Cofferdam will not run any other check against the file (the AST is unavailable). Fix the syntax error and re-run.

This finding cannot be suppressed via inline directives — there's no AST to associate the directive with. If you need to gate CI past these in legacy code, use `--baseline` to capture the current set.

### `Warning.TripleEquals`

| Field | Value |
|---|---|
| Priority | `15` |
| Severity | `high` |
| Type-aware | no |
| Options | _none_ |

`==` and `!=` perform type coercion and are almost always a bug. Use `===` and `!==` instead. Walks every `BinaryExpression` and flags the equality operators.

```ts
// flagged
if (a == b) return true;
if (a != b) return false;
```

```ts
// fix
if (a === b) return true;
if (a !== b) return false;
```

```ts
// not relevant — relational operators don't coerce in the same way
if (a < b) return true;
if (a >= b) return false;
```

```toml
# The default (high) gates CI — explicit override only useful to demote.
[checks."Warning.TripleEquals"]
severity = "medium"
```

## Suppression directives

All inline suppression shapes (line and block scope, all-checks or specific check IDs) — see [`examples/suppressions.ts`](../examples/suppressions.ts) for a full reference fixture.

```ts
// Next-line, all checks
// cofferdam-disable-next-line
if (a == b) { /* TripleEquals suppressed */ }

// Next-line, specific check(s)
// cofferdam-disable-next-line Warning.TripleEquals
if (x == y) { /* only TripleEquals suppressed */ }

// Block, all checks
/* cofferdam-disable */
if (c == d) { /* everything suppressed */ }
/* cofferdam-enable */

// Block, specific check IDs (comma-separated)
/* cofferdam-disable Warning.TripleEquals, Design.MaxParameters */
function example(a: number, b: number, c: number, d: number, e: number, f: number) {
  if (a == 0) return b;        // TripleEquals + MaxParameters both suppressed
}
/* cofferdam-enable */
```

`Warning.ParseError` cannot be suppressed inline — see its entry above.
