// AST view runtime tests (cd-svf). Verifies that the wire format
// reconstructed by the plugin host's `buildAstView` produces a typed
// AstView matching the `@cofferdam/check-sdk` interface.
//
// We exercise the host directly via a hand-built manifest rather than
// going through the Rust binary — this isolates the AST reconstruction
// logic from the parser and lets the test run cheaply on Node alone.
//
// The host script lives in `crates/cofferdam-cli/scripts/plugin-host.mjs`
// and is `include_str!`-bundled into the cofferdam binary; this test
// imports it directly to exercise its AST view code path.

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { pathToFileURL } from "node:url";

const HOST_PATH = new URL(
  "../../../crates/cofferdam-cli/scripts/plugin-host.mjs",
  import.meta.url,
).pathname.replace(/^\/([A-Z]:)/, "$1");

function runHost(manifest) {
  const result = spawnSync("node", [HOST_PATH], {
    input: JSON.stringify(manifest),
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`host exited ${result.status}: ${result.stderr}`);
  }
  return JSON.parse(result.stdout);
}

// Build a minimal plugin module on disk that exercises ast.findAll +
// ast.walk and reports one finding per matched node. Shared by the
// AST tests below.
function makeAstSmokePlugin(tmp) {
  // Plugin module needs an importable URL for the SDK. On Windows the
  // raw absolute path triggers "Received protocol 'c:'" — convert to a
  // file:// URL up front.
  const sdkUrl = new URL("../dist/index.js", import.meta.url).href;
  const pluginDir = mkdtempSync(join(tmp, "ast-smoke-"));
  const pluginFile = join(pluginDir, "index.mjs");
  writeFileSync(
    pluginFile,
    `
import { defineCheck, Category, Severity } from ${JSON.stringify(sdkUrl)};

export default defineCheck({
  id: "AstSmoke",
  category: Category.Warning,
  basePriority: 0,
  defaultSeverity: Severity.Low,
  explanation: "Reports one finding per (kind, identity) seen via findAll/walk.",
  run(file, ctx, _opts) {
    if (!file.ast) return;

    // findAll Pattern B
    for (const imp of file.ast.findAll("ImportDeclaration")) {
      ctx.report({ message: "import:" + imp.source, span: imp.span });
    }
    for (const call of file.ast.findAll("CallExpression")) {
      const callee = call.callee;
      const tag = callee?.kind === "MemberExpression"
        ? "member:" + (callee.property ?? "?")
        : callee?.kind === "IdentifierReference"
        ? "ident:" + callee.name
        : "other";
      ctx.report({ message: "call:" + tag, span: call.span });
    }

    // walk Pattern C — accumulate function names
    const funcs = [];
    file.ast.walk({
      visitFunction(node) {
        funcs.push(node.name ?? "<anon>");
        return "skip";
      },
    });
    if (funcs.length > 0) {
      ctx.report({ message: "funcs:" + funcs.join(","), span: file.ast.root.span });
    }
  },
});
`,
  );
  writeFileSync(join(pluginDir, "package.json"), JSON.stringify({ type: "module", main: "index.mjs" }));
  return pluginDir;
}

test("findAll + walk round-trip through wire format", () => {
  const tmp = mkdtempSync(join(tmpdir(), "cofferdam-ast-test-"));
  try {
    const pluginDir = makeAstSmokePlugin(tmp);

    // Hand-built wire matching what the Rust serializer emits for:
    //   import { foo } from "axios";
    //   axios.get("/api");
    const text = 'import { foo } from "axios";\naxios.get("/api");\n';
    const manifest = {
      cwd: tmp,
      plugins: [pluginDir],
      files: [
        {
          path: join(tmp, "smoke.ts"),
          text,
          lineViews: [],
          ast: {
            rootIdx: 0,
            nodes: [
              {
                kind: "Program",
                span: { line: 1, column: 1, start_byte: 0, end_byte: text.length },
                firstChild: 1,
                nextSibling: -1,
              },
              {
                kind: "ImportDeclaration",
                span: { line: 1, column: 1, start_byte: 0, end_byte: 28 },
                firstChild: -1,
                nextSibling: 2,
                source: "axios",
                specifiers: [{ localName: "foo", imported: "foo" }],
              },
              {
                kind: "CallExpression",
                span: { line: 2, column: 1, start_byte: 29, end_byte: 46 },
                firstChild: 3,
                nextSibling: -1,
                calleeIdx: 3,
                argumentIdxs: [],
              },
              {
                kind: "MemberExpression",
                span: { line: 2, column: 1, start_byte: 29, end_byte: 38 },
                firstChild: 4,
                nextSibling: -1,
                objectIdx: 4,
                property: "get",
                computed: false,
              },
              {
                kind: "IdentifierReference",
                span: { line: 2, column: 1, start_byte: 29, end_byte: 34 },
                firstChild: -1,
                nextSibling: -1,
                name: "axios",
              },
            ],
          },
        },
      ],
      options: {},
    };

    const { reports, errors } = runHost(manifest);
    assert.equal(errors.length, 0, `expected no host errors, got: ${JSON.stringify(errors)}`);

    const messages = reports.map((r) => r.message).sort();
    assert.deepEqual(messages, [
      "call:member:get",
      "import:axios",
    ].sort(), "findAll(ImportDeclaration) + findAll(CallExpression).callee shape");
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test("walk honours Walk.Skip", () => {
  const tmp = mkdtempSync(join(tmpdir(), "cofferdam-walk-test-"));
  try {
    const pluginDir = makeAstSmokePlugin(tmp);

    // Class containing a method (Function). visitFunction returns "skip"
    // so descent into the body is suppressed; nested CallExpressions
    // inside the method body should NOT trigger findAll-equivalent
    // reports via walk. (findAll is unaffected by Skip.)
    const text = 'class C { greet() { return frobnicate(); } }\n';
    const manifest = {
      cwd: tmp,
      plugins: [pluginDir],
      files: [
        {
          path: join(tmp, "skip.ts"),
          text,
          lineViews: [],
          ast: {
            rootIdx: 0,
            nodes: [
              {
                kind: "Program",
                span: { line: 1, column: 1, start_byte: 0, end_byte: text.length },
                firstChild: 1,
                nextSibling: -1,
              },
              {
                kind: "Class",
                span: { line: 1, column: 1, start_byte: 0, end_byte: 44 },
                firstChild: 2,
                nextSibling: -1,
                name: "C",
              },
              {
                kind: "Function",
                span: { line: 1, column: 11, start_byte: 10, end_byte: 42 },
                firstChild: 3,
                nextSibling: -1,
                name: "greet",
                paramIdxs: [],
                async: false,
                generator: false,
              },
              {
                kind: "CallExpression",
                span: { line: 1, column: 28, start_byte: 27, end_byte: 39 },
                firstChild: 4,
                nextSibling: -1,
                calleeIdx: 4,
                argumentIdxs: [],
              },
              {
                kind: "IdentifierReference",
                span: { line: 1, column: 28, start_byte: 27, end_byte: 38 },
                firstChild: -1,
                nextSibling: -1,
                name: "frobnicate",
              },
            ],
          },
        },
      ],
      options: {},
    };

    const { reports } = runHost(manifest);
    // walk emitted "funcs:greet" because visitFunction matched once.
    // Skip means we never descended into the method body — but findAll
    // still finds the inner CallExpression because findAll uses the
    // pre-built kind index, not the walk path.
    const messages = reports.map((r) => r.message);
    assert.ok(messages.includes("funcs:greet"), "walk visited the method");
    assert.ok(messages.includes("call:ident:frobnicate"), "findAll still sees inner call");
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});
