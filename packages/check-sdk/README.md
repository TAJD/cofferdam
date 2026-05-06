# @cofferdam/check-sdk

Author [cofferdam](https://github.com/TAJD/cofferdam) plugin checks in TypeScript.

`cofferdam` is a code-quality analyzer for TypeScript with a Rust core. The built-in checks cover the common ground (complexity, dead exports, triple-equals, etc); this SDK lets you express **project-specific architectural rules** as checks the same engine runs alongside the built-ins.

## Install

```sh
pnpm add -D @cofferdam/check-sdk     # pnpm
npm install -D @cofferdam/check-sdk  # npm
yarn add -D @cofferdam/check-sdk     # yarn
```

The SDK ships compiled JS + types; no runtime dependencies. Node 16+.

## Hello check

```ts
// my-checks/no-rovikore.ts
import { Category, defineCheck, Severity } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.High,
  explanation: "Brand must be spelled ROVIKORE.",
  files: { extensions: ["ts", "tsx"] },
  options: {
    brand: { default: "ROVIKORE", type: "string" },
  },
  run(file, ctx, opts) {
    for (const line of file.lines()) {
      // Pattern A — line walk. Skip comments and skip non-string content
      // so we don't false-positive on identifiers / JSX text.
      if (line.isComment || line.isJsxText) continue;
      if (!line.isStringLiteral) continue;
      const m = /\bRovikore\b/.exec(line.text);
      if (!m) continue;
      ctx.report({
        message: `Brand must be ${opts.brand}, not ${m[0]}.`,
        span: line.spanFor(m.index, m.index + m[0].length),
      });
    }
  },
});
```

Wire it into `cofferdam.toml`:

```toml
plugins = ["./my-checks/no-rovikore.ts"]

[checks."BrandCasing"]
brand = "ROVIKORE"
```

Then `cofferdam check src/` runs your plugin alongside every built-in check. Findings flow through the same priority computation, suppression directives, baseline diffing, and output formats.

## Three patterns

The SDK supports three check shapes, each suited to a different rule class:

- **Pattern A — line walk.** `for (const line of file.lines()) { … }`. Best for content checks against string literals or comments (brand spelling, banned words, license headers). The fastest path; no AST parse needed.
- **Pattern B — AST findAll.** `file.ast.findAll("CallExpression")`, `findAll("ImportDeclaration")`. Best when the rule is a tag-and-match against specific node kinds (banned imports, forbidden API calls).
- **Pattern C — stateful walk.** `file.ast.walk(visitor)` with accumulator state, decided per-file. Best when the rule depends on inter-statement context (tenant scoping across queries, accumulator-vs-mutator analysis).

`AstView` exposes 9 node kinds today: `Program`, `CallExpression`, `ImportDeclaration`, `Function`, `ArrowFunctionExpression`, `Class`, `ObjectExpression`, `MemberExpression`, `IdentifierReference`. The set is additive — new kinds can land in minor releases without breaking existing plugins.

## API surface

```ts
import {
  defineCheck,           // factory — returns a Check
  Category,              // Consistency | Design | Readability | Refactor | Warning
  Severity,              // Info | Low | Medium | High | Critical
  Walk,                  // visitor return: Continue | Skip
} from "@cofferdam/check-sdk";

import type {
  Check, CheckContext, ReportArgs, Fix,
  SourceFile, LineView, Span, RelatedSpan,
  AstView, AstVisitor, AstNode, NodeKind,
  // Per-kind typed nodes:
  ProgramNode, CallExpressionNode, ImportDeclarationNode,
  FunctionNode, ArrowFunctionExpressionNode, ClassNode,
  ObjectExpressionNode, MemberExpressionNode, IdentifierReferenceNode,
  // Options schema helpers:
  OptionKind, OptionSpec, OptionsSchema, ResolvedOptions,
} from "@cofferdam/check-sdk";
```

Everything in this list is part of the stability contract; new exports are additive. Removing or renaming an export is a breaking change.

## MemberExpression property resolution

`MemberExpressionNode` exposes two fields: `computed: boolean` and `property: string | undefined`.

- **Dot-form** (`obj.foo`, `computed=false`): `property` is always the identifier name — e.g. `"foo"`.
- **String-literal-indexed** (`obj["foo"]`, `computed=true`): `property` resolves to the literal value — e.g. `"foo"`. The resolver only applies when the index is a bare string literal; template literals and concatenation are treated as dynamic.
- **Dynamic-indexed** (`obj[runtimeVar]`, `computed=true`): `property` is `undefined`. The index is not statically determinable. Use `file.text.slice(node.span.start_byte, node.span.end_byte)` if you need the surface form.

In short: **`property` is always set for dot-form, and set for string-literal bracket-form. It is `undefined` only when the index is truly dynamic.**

```ts
for (const m of file.ast.findAll("MemberExpression")) {
  if (m.property === "fetch") {
    // Catches both obj.fetch and obj["fetch"]
    ctx.report({ message: "...", span: m.span });
  }
}
```

## ts-morph routing status (0.2.x)

`DefineCheckInput` exposes a `requiresTypes?: boolean` field for checks that need full type-aware analysis (resolved types, inferred generics, call signatures). **This field is reserved for future use in 0.2.x and is not yet wired.** Setting `requiresTypes: true` today is accepted by the type system and recorded on the `Check` object, but the engine does not route the check through ts-morph and no type information is available inside `run()`. The plugin host emits a one-time warning to stderr at load time when it encounters a plugin with `requiresTypes: true` so authors are not left wondering why type data is absent.

Type-aware routing via ts-morph is on the roadmap. Track cd-l58 / [gh #16](https://github.com/TAJD/cofferdam/issues/16) for status.

## Versioning

The SDK ships in lockstep with the cofferdam binary. `@cofferdam/check-sdk@X.Y.Z` is built and tested against `cofferdam@X.Y.Z`; the cofferdam plugin host enforces a major-version compatibility check at load time and refuses to run plugins whose vendored SDK is from a different major.

For 0.x: minor bumps may be breaking. Pin both packages to the same exact version until 1.0:

```jsonc
{
  "devDependencies": {
    "cofferdam": "0.2.2",
    "@cofferdam/check-sdk": "0.2.2"
  }
}
```

## Documentation

- Full plugin authoring guide (with worked examples for all three patterns): <https://tajd.github.io/cofferdam>
- AST surface reference: [`design/sdk-ast-surface.md`](https://github.com/TAJD/cofferdam/blob/main/design/sdk-ast-surface.md)
- AST wire format (Rust ↔ Node host): [`design/sdk-ast-wire.md`](https://github.com/TAJD/cofferdam/blob/main/design/sdk-ast-wire.md)
- Three end-to-end fixtures (working plugins): [`examples-plugins/`](https://github.com/TAJD/cofferdam/tree/main/examples-plugins)

## License

MIT.
