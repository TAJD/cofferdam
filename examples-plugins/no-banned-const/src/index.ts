// NoBannedConst — flags a `const`-bound identifier whose name is on a
// banned list (CD-78 e2e fixture). Pattern B / AST findAll.
//
// The motivating case (CD-69's NoLocalBuildHeaders): a project bans
// re-implementing `buildHeaders` locally. `function buildHeaders() {}`
// is already findable via findAll("Function"), but the modern
// `const buildHeaders = (...) => {}` form was invisible to the AST
// surface until CD-78 added `VariableDeclaration`. This check closes
// that gap by walking `const` declarators and matching their names.

import {
  Category,
  defineCheck,
  Severity,
} from "@cofferdam/check-sdk";

const DEFAULT_BANNED = ["buildHeaders"];

export default defineCheck({
  id: "NoBannedConst",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.High,
  explanation:
    "Re-declaring a reserved project identifier as a local `const` " +
    "shadows the shared implementation. Import the canonical one " +
    "instead of binding the name locally.",
  options: {
    bannedNames: { default: DEFAULT_BANNED, type: "string[]" },
  },
  files: {
    extensions: ["ts", "tsx", "mts", "cts"],
  },
  run(file, ctx, opts) {
    if (!file.ast) return;

    const banned = new Set(opts.bannedNames);

    for (const decl of file.ast.findAll("VariableDeclaration")) {
      if (decl.declarationKind !== "const") continue;

      for (const d of decl.declarations) {
        if (d.name === undefined || !banned.has(d.name)) continue;

        const boundToFn =
          d.init?.kind === "ArrowFunctionExpression" || d.init?.kind === "Function";

        ctx.report({
          message: boundToFn
            ? `Local const \`${d.name}\` shadows a banned identifier (init kind: ${d.init?.kind}).`
            : `Local const \`${d.name}\` uses a banned identifier name.`,
          span: decl.span,
        });
      }
    }
  },
});
