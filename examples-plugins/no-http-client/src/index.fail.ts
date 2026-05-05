// EXPECTED to fail tsc when any `@ts-expect-error` directive stops
// firing — e.g. if cd-svf's AST surface loosens. This file pins the
// type tightness on the AST shape used by the NoHttpClient plugin
// (cd-81a.2 / cd-svf acceptance — different surface than BrandCasing's
// fail fixture, which exercises LineView + opts).

import { Category, defineCheck, type SourceFile, type CheckContext } from "@cofferdam/check-sdk";

declare const file: SourceFile;
declare const ctx: CheckContext;

export default defineCheck({
  id: "NoHttpClient",
  category: Category.Warning,
  basePriority: 15,
  explanation: "x",
  run() {
    if (!file.ast) return;

    // findAll narrows to the requested kind. Asking for a kind not in
    // NodeKind must fail to compile.
    // @ts-expect-error
    file.ast.findAll("Calls");

    // ImportDeclaration's `source` field is a string. Treating it as a
    // number must fail.
    for (const imp of file.ast.findAll("ImportDeclaration")) {
      // @ts-expect-error
      const _len: number = imp.source;
      void _len;
    }

    // CallExpression's `callee` field is an AstNode (typed object), not
    // a number.
    for (const call of file.ast.findAll("CallExpression")) {
      // @ts-expect-error
      const _bad: number = call.callee;
      void _bad;
    }

    // Wrong report shape — span is required.
    // @ts-expect-error
    ctx.report({ message: "missing span" });
  },
});
