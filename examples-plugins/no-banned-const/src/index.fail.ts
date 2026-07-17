// EXPECTED to fail tsc when any `@ts-expect-error` directive stops
// firing — e.g. if CD-78's VariableDeclaration surface loosens. This
// pins the type tightness on the `VariableDeclaration` node shape.

import { Category, defineCheck, type SourceFile, type CheckContext } from "@cofferdam/check-sdk";

declare const file: SourceFile;
declare const ctx: CheckContext;

export default defineCheck({
  id: "NoBannedConst",
  category: Category.Warning,
  basePriority: 15,
  explanation: "x",
  run() {
    if (!file.ast) return;

    for (const decl of file.ast.findAll("VariableDeclaration")) {
      // `declarationKind` is a `"const" | "let" | "var"` union, not a number.
      // @ts-expect-error
      const _k: number = decl.declarationKind;
      void _k;

      for (const d of decl.declarations) {
        // `name` is `string | undefined`, not a number.
        // @ts-expect-error
        const _n: number = d.name;
        void _n;

        // `init` is `AstNode | undefined` (a typed object), not a string.
        // @ts-expect-error
        const _i: string = d.init;
        void _i;
      }
    }

    // Wrong report shape — span is required.
    // @ts-expect-error
    ctx.report({ message: "missing span" });
  },
});
