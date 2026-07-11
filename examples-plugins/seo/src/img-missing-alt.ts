// SeoImgMissingAlt — JSX findAll pattern (CD-80).
//
// Ported from docs/plugin-sdk-guide.md's "JSX findAll — flag <img> with
// no alt" worked example; renamed to namespace it under this package.

import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "SeoImgMissingAlt",
  category: Category.Warning,
  basePriority: 10,
  defaultSeverity: Severity.Medium,
  explanation: "An <img> element has no `alt` attribute, hurting accessibility and image SEO.",
  files: { extensions: ["tsx", "jsx"] },
  run(file, ctx) {
    if (!file.ast) return;
    for (const el of file.ast.findAll("JSXElement")) {
      if (el.tagName !== "img") continue;
      const hasAlt = el.attributes.some((a) => a.isSpread || a.name === "alt");
      if (!hasAlt) {
        ctx.report({ message: "<img> is missing an `alt` attribute", span: el.span });
      }
    }
  },
});
