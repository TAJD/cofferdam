// Type-level acceptance tests for @cofferdam/check-sdk.
//
// Run via `npx tsc -p tests` from the package root. Each `expectType<T>(v)`
// pin asserts the inferred type without producing runtime code; failures
// surface as compile errors. The negative cases live in `negative.ts`.

import {
  defineCheck,
  Category,
  Severity,
  Walk,
  type Check,
  type CheckContext,
  type ResolvedOptions,
  type SourceFile,
} from "../src/index.js";

// --- type-level assert helper ---

declare function expectType<T>(value: T): void;
type Equals<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2
  ? true
  : false;
declare function expectEqual<A, B>(eq: Equals<A, B>): void;

// --- 1. defineCheck infers options type into run's third arg ---

const branded = defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  explanation: "Brand name must be all-caps.",
  options: {
    brand: { default: "ROVIKORE", type: "string" },
    allowedAliases: { default: [] as string[], type: "string[]" },
    maxHits: { default: 5, type: "number" },
    requireExempt: { default: false, type: "boolean" },
  },
  run(_file, _ctx, opts) {
    expectType<string>(opts.brand);
    expectType<readonly string[]>(opts.allowedAliases);
    expectType<number>(opts.maxHits);
    expectType<boolean>(opts.requireExempt);
  },
});

expectType<Check<typeof branded.options>>(branded);

// --- 2. defineCheck without options gives an empty opts bag ---

const noOpts = defineCheck({
  id: "MyCheck",
  category: Category.Refactor,
  basePriority: 10,
  explanation: "x",
  run(_file, _ctx, opts) {
    expectEqual<typeof opts, ResolvedOptions<{}>>(true);
  },
});
expectType<Check>(noOpts);

// --- 3. Category and Severity are string-literal unions, not opaque ---

expectEqual<Category, "consistency" | "design" | "readability" | "refactor" | "warning">(true);
expectEqual<Severity, "info" | "low" | "medium" | "high" | "critical">(true);
expectEqual<Walk, "continue" | "skip">(true);

// --- 4. CheckContext.report accepts the canonical args shape ---

declare const ctx: CheckContext;
declare const file: SourceFile;
const ln = file.lines().next().value!;
ctx.report({
  message: "x",
  span: ln.spanFor(0, 4),
});
ctx.report({
  message: "x",
  span: ln.spanFor(0, 4),
  severity: Severity.Critical,
  related: [{ file: "other.ts", span: ln.spanFor(1, 2) }],
});

// --- 5. AST findAll narrows to the requested kind ---

if (file.ast) {
  const calls = file.ast.findAll("CallExpression");
  // calls is readonly CallExpressionNode[] — kind is the literal.
  for (const c of calls) {
    expectEqual<typeof c.kind, "CallExpression">(true);
  }
  const imports = file.ast.findAll("ImportDeclaration");
  for (const i of imports) {
    expectType<string>(i.source);
  }
}

// --- cd-xlv: options round-trip — defaults survive into run() ---

const optsRoundTrip = defineCheck({
  id: "OptsTest",
  category: Category.Refactor,
  basePriority: 5,
  explanation: "x",
  options: {
    foo: { default: "bar", type: "string" },
    items: { default: [] as string[], type: "string[]" },
    limit: { default: 10, type: "number" },
    enabled: { default: true, type: "boolean" },
    coords: { default: [0, 0] as number[], type: "number[]" },
  },
  run(_file, _ctx, opts) {
    expectType<string>(opts.foo);
    expectType<readonly string[]>(opts.items);
    expectType<number>(opts.limit);
    expectType<boolean>(opts.enabled);
    expectType<readonly number[]>(opts.coords);
  },
});

// At runtime, the returned check carries the schema verbatim — the
// loader's runPlugin reads `check.options[key].default` to produce the
// `opts` arg when no cofferdam.toml override is in play.
expectType<typeof optsRoundTrip.options>(optsRoundTrip.options);
expectType<"bar">(optsRoundTrip.options.foo.default);
expectType<10>(optsRoundTrip.options.limit.default);
expectType<true>(optsRoundTrip.options.enabled.default);

// --- cd-l4c: FileScope accepts both pathPattern (deprecated singular)
//             and pathPatterns (plural). Authors can use either form,
//             or both, while the singular is being phased out.

const scopedSingular = defineCheck({
  id: "Scoped.Singular",
  category: Category.Warning,
  basePriority: 5,
  explanation: "x",
  files: {
    extensions: ["ts", "tsx"],
    pathPattern: "src/**/*",
    excludePatterns: ["**/__tests__/**"],
  },
  run() {},
});
expectType<Check>(scopedSingular);

const scopedPlural = defineCheck({
  id: "Scoped.Plural",
  category: Category.Warning,
  basePriority: 5,
  explanation: "x",
  files: {
    extensions: ["ts", "tsx"],
    pathPatterns: ["src/api/**", "src/server/**", "lib/**/*.ts"],
    excludePatterns: ["**/__tests__/**"],
  },
  run() {},
});
expectType<Check>(scopedPlural);

// Both forms together — semantics: file matches if it matches the
// singular OR any plural entry. Type system accepts the combination.
const scopedBoth = defineCheck({
  id: "Scoped.Both",
  category: Category.Warning,
  basePriority: 5,
  explanation: "x",
  files: {
    pathPattern: "legacy/**",
    pathPatterns: ["src/**", "lib/**"],
  },
  run() {},
});
expectType<Check>(scopedBoth);
