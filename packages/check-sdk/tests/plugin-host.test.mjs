// Runtime test for the plugin host. Builds a real Check via defineCheck,
// hands it a synthetic file with the line-view shape the napi layer
// produces, and asserts the reports come back with file-absolute byte
// offsets that round-trip back to the trigger string.
//
// Run: node --test tests/plugin-host.test.mjs (after `npm run build`)

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const ROOT = dirname(fileURLToPath(import.meta.url));
const PKG = resolve(ROOT, "..");

// Compile the SDK first so we can import from dist/.
test("setup: build the SDK", () => {
  execFileSync("npx", ["tsc", "-p", "."], { cwd: PKG, encoding: "utf8", shell: true });
});

test("runPlugin emits one report per matched line, with file-absolute spans", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  const trigger = /\bRovikore\b/;
  const check = defineCheck({
    id: "BrandCasing",
    category: Category.Warning,
    basePriority: 15,
    explanation: "Brand must be ROVIKORE.",
    options: {
      brand: { default: "ROVIKORE", type: "string" },
    },
    run(file, ctx, opts) {
      for (const ln of file.lines()) {
        if (ln.isComment || ln.isDocComment || ln.isPragma) continue;
        if (!ln.isStringLiteral && !ln.isJsxText) continue;
        const m = trigger.exec(ln.text);
        if (!m) continue;
        ctx.report({
          message: `Brand must be ${opts.brand}, not ${m[0]}.`,
          span: ln.spanFor(m.index, m.index + m[0].length),
        });
      }
    },
  });

  const text = "const x = 1;\nconst y = 'Rovikore';\nconst z = 2;\n";
  // Synthetic line views matching the cofferdam-core LineView shape.
  // `lineStart` reflects each line's byte offset in `text`.
  const lineViews = [
    { lineNo: 1, text: "const x = 1;",        isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 },
    { lineNo: 2, text: "const y = 'Rovikore';", isComment: false, isDocComment: false, isStringLiteral: true,  isJsxText: false, isPragma: false, lineStart: 13 },
    { lineNo: 3, text: "const z = 2;",        isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 36 },
  ];

  const reports = runPlugin(check, { path: "synthetic.ts", text, lineViews });

  assert.equal(reports.length, 1);
  const r = reports[0];
  assert.equal(r.checkId, "BrandCasing");
  assert.equal(r.file, "synthetic.ts");
  // Line 2 is "const y = 'Rovikore';" — 'Rovikore' starts at char 11 in
  // the line text, so file byte = 13 + 11 = 24, end = 24 + 8 = 32.
  assert.equal(r.startByte, 24);
  assert.equal(r.endByte, 32);
  // Round-trip: slice the text by the reported bytes.
  assert.equal(text.slice(r.startByte, r.endByte), "Rovikore");
});

test("runPlugin captures plugin throws as Warning.PluginCrashed", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  const check = defineCheck({
    id: "Misbehaving",
    category: Category.Warning,
    basePriority: 10,
    explanation: "x",
    run() {
      throw new Error("boom");
    },
  });

  const reports = runPlugin(check, {
    path: "f.ts",
    text: "x",
    lineViews: [{ lineNo: 1, text: "x", isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 }],
  });

  assert.equal(reports.length, 1);
  assert.equal(reports[0].checkId, "Warning.PluginCrashed");
  assert.match(reports[0].message, /boom/);
});

test("loadPlugins rejects non-Check default exports", async () => {
  const { loadPlugins } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  // Build a temp file that exports a non-Check default.
  const fs = await import("node:fs");
  const os = await import("node:os");
  const tmp = fs.mkdtempSync(resolve(os.tmpdir(), "cofferdam-loader-"));
  const path = resolve(tmp, "bad.mjs");
  fs.writeFileSync(path, "export default { not: 'a check' };\n");

  await assert.rejects(
    () => loadPlugins([path]),
    /did not default-export a Check/,
  );

  fs.rmSync(tmp, { recursive: true, force: true });
});
