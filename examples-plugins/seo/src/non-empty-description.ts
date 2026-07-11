// SeoNonEmptyDescription — cross-file literal resolution (CD-82).
//
// Ported from docs/plugin-sdk-guide.md's "Resolving imported literals
// (ctx.types.resolveLiteral)" worked example; renamed to namespace it
// under this package. Matches `IdentifierReference` nodes literally
// named "description" — fixture files importing a shared constant into
// their `metadata` object must alias it to that exact name at the use
// site (see fixture/page-empty-description.tsx).

import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "SeoNonEmptyDescription",
  category: Category.Warning,
  basePriority: 10,
  defaultSeverity: Severity.Medium,
  explanation: "SEO description metadata should not be empty.",
  requiresTypes: true,
  files: { extensions: ["ts", "tsx"] },
  async run(file, ctx) {
    if (!file.ast || !ctx.types) return;
    for (const id of file.ast.findAll("IdentifierReference")) {
      if (id.name !== "description") continue;
      const facts = await ctx.types.resolveLiteral(id.span.start_byte, id.span.end_byte);
      if (facts?.literalString !== undefined && facts.literalString.trim() === "") {
        ctx.report({ message: "SEO description resolves to an empty string.", span: id.span });
      }
    }
  },
});
