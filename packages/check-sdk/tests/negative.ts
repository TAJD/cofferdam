// Negative type-level tests. Each `@ts-expect-error` line asserts that
// the SDK rejects the construct on its line. If a test ever stops
// failing (e.g. types loosened), tsc reports the directive itself as
// unused — which fails this file.

import {
  defineCheck,
  Category,
  type CheckContext,
  type SourceFile,
} from "../src/index.js";

// --- 1. Wrong category literal ---

defineCheck({
  id: "X",
  // @ts-expect-error — "warnings" is not a Category member
  category: "warnings",
  basePriority: 10,
  explanation: "x",
  run() {},
});

// --- 2. Wrong AST property access (the canonical cd-7e4 negative-fixture case) ---

declare const file: SourceFile;
if (file.ast) {
  // @ts-expect-error — `.lyne` typo; the method is `.lines()` on SourceFile
  file.lyne();
  // @ts-expect-error — findAll requires a NodeKind; "Calls" is not one
  file.ast.findAll("Calls");
}

// --- 3. opts type rejects wrong member access ---

defineCheck({
  id: "X",
  category: Category.Warning,
  basePriority: 15,
  explanation: "x",
  options: {
    brand: { default: "X", type: "string" },
  },
  run(_file, _ctx, opts) {
    // @ts-expect-error — `notDeclared` is not in the schema
    opts.notDeclared;
    // @ts-expect-error — opts.brand is string, not number
    const _n: number = opts.brand;
  },
});

// --- 4. ctx.report rejects missing span ---

declare const ctx: CheckContext;
// @ts-expect-error — span is required
ctx.report({ message: "x" });
