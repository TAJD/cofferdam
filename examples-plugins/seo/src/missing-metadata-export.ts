// SeoMissingMetadataExport — text/regex heuristic (Pattern A).
//
// The frozen plugin AST surface (packages/check-sdk/src/ast.ts) has no
// `ExportNamedDeclaration` node kind, so "does this file export X" can't
// be answered via a typed AST node today — a regex over the source text
// is the only option. Flags a Next.js App Router `page.tsx`/`page.ts`
// that has neither `export const metadata` nor `export function
// generateMetadata` / `export async function generateMetadata`.

import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";

const HAS_METADATA = /export\s+(const|let)\s+metadata\b/;
const HAS_GENERATE_METADATA = /export\s+(async\s+)?function\s+generateMetadata\b/;

export default defineCheck({
  id: "SeoMissingMetadataExport",
  category: Category.Warning,
  basePriority: 12,
  defaultSeverity: Severity.Medium,
  explanation:
    "A page component has no `metadata` or `generateMetadata` export, so it has no page title/description for search engines or social previews.",
  files: { extensions: ["ts", "tsx"], pathPatterns: ["**/page.ts", "**/page.tsx"] },
  run(file, ctx) {
    if (HAS_METADATA.test(file.text) || HAS_GENERATE_METADATA.test(file.text)) return;
    ctx.report({
      message: "Page component has no `metadata` or `generateMetadata` export.",
      span: { line: 1, column: 1, start_byte: 0, end_byte: Math.min(1, file.text.length) },
    });
  },
});
