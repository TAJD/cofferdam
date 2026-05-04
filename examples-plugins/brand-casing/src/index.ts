// BrandCasing — port of rovikore-host's Credo check (cd-7e4 e2e
// fixture). Flags any user-facing occurrence of "Rovikore" — the brand
// is supposed to render in all caps everywhere copy reaches users.
//
// Lines exempt from the flag:
//   - any comment / JSDoc / pragma (covered by file.lines() flags)
//   - lines whose previous line carries `// brand:ignore — <why>` (the
//     plugin-defined escape hatch — separate from engine-level
//     `// cofferdam-ignore: BrandCasing` which the engine filters
//     after this plugin emits)
//   - identifier-only lines (no string literal or JSX text)
//   - allowlisted aliases configured via cofferdam.toml

import { Category, defineCheck, Severity } from "@cofferdam/check-sdk";

const TRIGGER = /\bRovikore\b/;
const PLUGIN_IGNORE = /\bbrand:ignore\b/;

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.High,
  explanation:
    'Brand name must be all-caps "ROVIKORE" in user-facing copy. ' +
    "Add `// brand:ignore — <why>` on the previous line for legitimate references.",
  options: {
    brand: { default: "ROVIKORE", type: "string" },
    allowedAliases: { default: [] as string[], type: "string[]" },
  },
  files: {
    extensions: ["ts", "tsx", "mts", "cts"],
  },
  run(file, ctx, opts) {
    const lines = [...file.lines()];

    // Pre-pass: collect the line numbers that have `// brand:ignore`
    // on the line BEFORE them. Plugin-level escape hatch (cd-cmb).
    const ignoredNext = new Set<number>();
    for (const ln of lines) {
      if (PLUGIN_IGNORE.test(ln.text)) ignoredNext.add(ln.lineNo + 1);
    }

    for (const ln of lines) {
      if (ignoredNext.has(ln.lineNo)) continue;
      if (ln.isComment || ln.isDocComment || ln.isPragma) continue;
      if (!ln.isStringLiteral && !ln.isJsxText) continue;

      const m = TRIGGER.exec(ln.text);
      if (!m) continue;

      // Allowlist check — exact-substring match keeps the API minimal.
      if (opts.allowedAliases.some((a) => ln.text.includes(a))) continue;

      ctx.report({
        message: `Brand name must be "${opts.brand}", not "${m[0]}".`,
        span: ln.spanFor(m.index, m.index + m[0].length),
      });
    }
  },
});
