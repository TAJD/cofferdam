// ScopeGlobProbe — regression fixture for CD-70 (cofferdam-cli/scripts/
// plugin-host.mjs's `globMatchSingle`): a trailing `**` in a plugin's
// `files.pathPatterns` (e.g. `widgets/**`) must match a file directly
// inside the directory, not only files nested another level down.
//
// `files.pathPatterns: ["widgets/**"]` scopes this check to
// `fixture/widgets/**`. The fixture includes a file directly in
// `widgets/` (`direct.ts`), a nested file (`nested/deep.ts`), and a file
// outside the scope (`other/skip.ts`) that must NOT be flagged.

import { Category, defineCheck, Severity } from "@cofferdam/check-sdk";

const MARKER = /SCOPE-PROBE/;

export default defineCheck({
  id: "ScopeGlobProbe",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.Medium,
  explanation: "Fixture marker found inside the scoped directory.",
  files: {
    extensions: ["ts"],
    pathPatterns: ["widgets/**"],
  },
  run(file, ctx) {
    for (const ln of file.lines()) {
      const m = MARKER.exec(ln.text);
      if (!m) continue;
      ctx.report({
        message: "SCOPE-PROBE marker found in scoped file.",
        span: ln.spanFor(m.index, m.index + m[0].length),
      });
    }
  },
});
