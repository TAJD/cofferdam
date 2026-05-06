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

test("cd-xlv: schema defaults survive round-trip into run()", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  // Capture the opts arg the run() callback received so we can assert
  // on its shape AFTER the plugin host has materialised it.
  /** @type {unknown} */
  let captured;
  const check = defineCheck({
    id: "OptsRoundTrip",
    category: Category.Refactor,
    basePriority: 5,
    explanation: "x",
    options: {
      foo: { default: "bar", type: "string" },
      items: { default: ["a", "b"], type: "string[]" },
      limit: { default: 10, type: "number" },
      enabled: { default: true, type: "boolean" },
    },
    run(_file, _ctx, opts) {
      captured = opts;
    },
  });

  runPlugin(check, {
    path: "f.ts",
    text: "x",
    lineViews: [{ lineNo: 1, text: "x", isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 }],
  });

  assert.deepEqual(captured, {
    foo: "bar",
    items: ["a", "b"],
    limit: 10,
    enabled: true,
  });
});

test("cd-5tz: layer field propagates into file.layer when input has layer set", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  /** @type {string | null | undefined} */
  let capturedLayer;
  const check = defineCheck({
    id: "LayerCapture",
    category: Category.Warning,
    basePriority: 5,
    explanation: "capture layer for test",
    run(file, _ctx) {
      capturedLayer = file.layer;
    },
  });

  runPlugin(check, {
    path: "src/ui/button.ts",
    text: "export const x = 1;",
    lineViews: [{ lineNo: 1, text: "export const x = 1;", isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 }],
    layer: "ui",
  });

  assert.equal(capturedLayer, "ui");
});

test("cd-5tz: layer field is null when input layer is null", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  /** @type {string | null | undefined} */
  let capturedLayer;
  const check = defineCheck({
    id: "LayerCaptureNull",
    category: Category.Warning,
    basePriority: 5,
    explanation: "capture null layer for test",
    run(file, _ctx) {
      capturedLayer = file.layer;
    },
  });

  runPlugin(check, {
    path: "scripts/build.ts",
    text: "const x = 1;",
    lineViews: [{ lineNo: 1, text: "const x = 1;", isComment: false, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 0 }],
    layer: null,
  });

  assert.equal(capturedLayer, null);
});

test("cd-4if: FileScope.layers pre-filters before run() is invoked", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  let runCalls = 0;
  const check = defineCheck({
    id: "UiOnly",
    category: Category.Warning,
    basePriority: 5,
    explanation: "ui layer only",
    files: { layers: ["ui"] },
    run(_file, ctx) {
      runCalls++;
      ctx.report({
        message: "x",
        span: { line: 1, column: 1, start_byte: 0, end_byte: 1 },
      });
    },
  });

  const baseInput = (layer) => ({
    path: "src/anywhere/file.ts",
    text: "x",
    lineViews: [
      {
        lineNo: 1,
        text: "x",
        isComment: false,
        isDocComment: false,
        isStringLiteral: false,
        isJsxText: false,
        isPragma: false,
        lineStart: 0,
      },
    ],
    layer,
  });

  // In-layer: run() fires, one report.
  assert.equal(runPlugin(check, baseInput("ui")).length, 1);
  // Wrong layer: pre-filter blocks run() entirely.
  assert.equal(runPlugin(check, baseInput("lib")).length, 0);
  // No layer: file outside every declared layer is excluded by a non-empty `layers` filter.
  assert.equal(runPlugin(check, baseInput(null)).length, 0);
  // run() should have fired exactly once across the three calls (the in-layer one).
  assert.equal(runCalls, 1);
});

test("cd-4if: FileScope.layers omitted/empty applies no layer filter", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  const check = defineCheck({
    id: "AnyLayer",
    category: Category.Warning,
    basePriority: 5,
    explanation: "no layer filter",
    files: { extensions: ["ts"] },
    run(_file, ctx) {
      ctx.report({
        message: "x",
        span: { line: 1, column: 1, start_byte: 0, end_byte: 1 },
      });
    },
  });

  const input = (layer) => ({
    path: "src/anywhere/file.ts",
    text: "x",
    lineViews: [
      {
        lineNo: 1,
        text: "x",
        isComment: false,
        isDocComment: false,
        isStringLiteral: false,
        isJsxText: false,
        isPragma: false,
        lineStart: 0,
      },
    ],
    layer,
  });

  assert.equal(runPlugin(check, input("ui")).length, 1);
  assert.equal(runPlugin(check, input("lib")).length, 1);
  assert.equal(runPlugin(check, input(null)).length, 1);
});

test("plugin magic comments filter inside run(); engine suppression filters at the engine boundary", async () => {
  const { defineCheck, Category, runPlugin } = await import(
    `file://${PKG.replace(/\\/g, "/")}/dist/index.js`
  );

  const trigger = /\bRovikore\b/;

  // Plugin defines its own magic-comment escape hatch — `// brand:ignore`
  // on the previous line. This filtering runs INSIDE the plugin (cd-cmb
  // precedence: layer 2). Engine suppression (`// cofferdam-ignore`)
  // happens AFTER the plugin emits — that's a separate test against the
  // engine in Rust.
  const check = defineCheck({
    id: "BrandCasing",
    category: Category.Warning,
    basePriority: 15,
    explanation: "x",
    run(file, ctx) {
      const lines = [...file.lines()];
      const ignored = new Set();
      for (const ln of lines) {
        if (/\bbrand:ignore\b/.test(ln.text)) ignored.add(ln.lineNo + 1);
      }
      for (const ln of lines) {
        if (ignored.has(ln.lineNo)) continue;
        if (!ln.isStringLiteral) continue;
        const m = trigger.exec(ln.text);
        if (!m) continue;
        ctx.report({
          message: "brand",
          span: ln.spanFor(m.index, m.index + m[0].length),
        });
      }
    },
  });

  const text = [
    "const a = 'Rovikore one';",                        // line 1 — FLAG
    "// brand:ignore — legacy fixture",                 // line 2 — magic comment
    "const b = 'Rovikore two';",                        // line 3 — EXEMPT (plugin filter)
    "const c = 'Rovikore three';",                      // line 4 — FLAG
  ].join("\n") + "\n";

  const lineViews = [
    { lineNo: 1, text: "const a = 'Rovikore one';",  isComment: false, isDocComment: false, isStringLiteral: true,  isJsxText: false, isPragma: false, lineStart: 0 },
    { lineNo: 2, text: "// brand:ignore — legacy fixture", isComment: true, isDocComment: false, isStringLiteral: false, isJsxText: false, isPragma: false, lineStart: 26 },
    { lineNo: 3, text: "const b = 'Rovikore two';",  isComment: false, isDocComment: false, isStringLiteral: true,  isJsxText: false, isPragma: false, lineStart: 60 },
    { lineNo: 4, text: "const c = 'Rovikore three';", isComment: false, isDocComment: false, isStringLiteral: true,  isJsxText: false, isPragma: false, lineStart: 86 },
  ];

  const reports = runPlugin(check, { path: "f.ts", text, lineViews });

  // Two reports: lines 1 and 4. Line 3 was filtered by the plugin's own
  // magic-comment logic before ctx.report fired.
  assert.equal(reports.length, 2);
  // The first should be at line 1 — span starts at byte 11 + 0 = 11.
  assert.equal(reports[0].startByte, 11);
  // The second at line 4.
  assert.ok(reports[1].startByte > 86);
});
