// EXPECTED to fail tsc when any `@ts-expect-error` directive stops
// firing — e.g. if cd-svf's AST surface loosens. This file pins type
// tightness on the AstView.walk visitor surface (cd-81a.2 / cd-svf
// acceptance — different surface than BrandCasing's LineView fail
// fixture and NoHttpClient's findAll fail fixture).

import {
  Category,
  defineCheck,
  Walk,
  type AstVisitor,
  type CallExpressionNode,
  type CheckContext,
  type ImportDeclarationNode,
  type SourceFile,
} from "@cofferdam/check-sdk";

declare const file: SourceFile;
declare const ctx: CheckContext;

export default defineCheck({
  id: "TenantIsolation",
  category: Category.Warning,
  basePriority: 15,
  explanation: "x",
  run() {
    if (!file.ast) return;

    // CallExpressionNode has no `source` field — accessing it on the
    // typed visitor parameter must fail.
    const badFieldAccess: AstVisitor = {
      visitCallExpression(node: CallExpressionNode): Walk {
        // @ts-expect-error
        const _src: string = node.source;
        void _src;
        return Walk.Continue;
      },
    };
    void badFieldAccess;

    // Return type Walk excludes arbitrary strings. Building a visitor
    // whose visit method returns a non-Walk value must fail at the
    // property-assignment site.
    const badReturn: AstVisitor = {
      visitImportDeclaration(_node: ImportDeclarationNode): Walk {
        // @ts-expect-error — `"keep"` is not assignable to Walk
        return "keep";
      },
    };
    void badReturn;

    // ctx.report still requires span (sanity check that the constraint
    // hasn't relaxed when adding visitor support).
    // @ts-expect-error
    ctx.report({ message: "missing span" });
  },
});
