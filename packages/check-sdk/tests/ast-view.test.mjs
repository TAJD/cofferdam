// AST view runtime tests (cd-svf). Verifies that the wire format
// reconstructed by the plugin host's `buildAstView` produces a typed
// AstView matching the `@cofferdam/check-sdk` interface.
//
// We exercise the host directly via hand-built NDJSON records rather than
// going through the Rust binary — this isolates the AST reconstruction
// logic from the parser and lets the test run cheaply on Node alone.
//
// The host script lives in `crates/cofferdam-cli/scripts/plugin-host.mjs`
// and is `include_str!`-bundled into the cofferdam binary; this test
// spawns it directly (CD-33 streaming NDJSON protocol — header/file/end
// records in, report/error/done records out; see `design/sdk-ast-wire.md`)
// to exercise its AST view code path. `runHost` mirrors the helper in
// `plugin-host-types.test.mjs`.

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const ROOT = dirname(fileURLToPath(import.meta.url));
const HOST_PATH = resolve(ROOT, "..", "..", "..", "crates", "cofferdam-cli", "scripts", "plugin-host.mjs");

/** Drive the host script over stdin/stdout with the given NDJSON records. */
function runHost(records) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, [HOST_PATH], { stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d.toString()));
    child.stderr.on("data", (d) => (stderr += d.toString()));
    child.on("error", reject);
    child.on("close", () => {
      const lines = stdout
        .split("\n")
        .map((l) => l.trim())
        .filter(Boolean)
        .map((l) => JSON.parse(l));
      resolvePromise({ lines, stderr });
    });
    for (const rec of records) {
      child.stdin.write(JSON.stringify(rec) + "\n");
    }
    child.stdin.end();
  });
}

/** Drive the host with a header (one plugin dir, no options) + a single
 * file record carrying `ast`, then `end` — the shape every test below
 * needs. Returns `{reports, errors}` collected from the streamed lines. */
async function runAstFixture(pluginDir, cwd, filePath, text, ast) {
  const { lines } = await runHost([
    {
      type: "header",
      wireVersion: 2,
      cwd: cwd.replace(/\\/g, "/"),
      plugins: [pluginDir.replace(/\\/g, "/")],
      options: {},
    },
    {
      type: "file",
      path: filePath.replace(/\\/g, "/"),
      text,
      lineViews: [],
      layer: null,
      ast,
    },
    { type: "end" },
  ]);
  return {
    reports: lines.filter((l) => l.type === "report"),
    errors: lines.filter((l) => l.type === "error"),
  };
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

test("findAll + walk round-trip through wire format", async () => {
  const tmp = mkdtempSync(join(tmpdir(), "cofferdam-ast-test-"));
  try {
    const pluginDir = makeAstSmokePlugin(tmp);

    // Hand-built wire matching what the Rust serializer emits for:
    //   import { foo } from "axios";
    //   axios.get("/api");
    const text = 'import { foo } from "axios";\naxios.get("/api");\n';
    const ast = {
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
    };

    const { reports, errors } = await runAstFixture(pluginDir, tmp, join(tmp, "smoke.ts"), text, ast);
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

// cd-5kc: MemberExpression.computed coverage.
// Verifies the four (computed, property) cases the Rust wire builder
// emits for member-access expressions:
//
//   const a = Math.random();         // computed=false, property="random"
//   const b = Math["random"]();      // computed=true,  property="random"
//   const c = process["env"]["FOO"]; // computed=true,  property="env" (inner)
//                                    // computed=true,  property="FOO" (outer)
//   const k = "random";
//   const d = Math[k]();             // computed=true,  property=undefined
//
// The Rust serializer (ast_wire.rs::visit_member_expression) resolves
// `property` for ComputedMemberExpression only when the index expression
// is a StringLiteral; dynamic expressions stay undefined. StaticMember
// (dot-form) always resolves. This test exercises the hand-built wire
// format directly so it runs without the Rust binary.
test("MemberExpression.computed coverage — static, string-literal, chained, dynamic", async () => {
  const tmp = mkdtempSync(join(tmpdir(), "cofferdam-member-test-"));
  try {
    const sdkUrl = new URL("../dist/index.js", import.meta.url).href;
    const pluginDir = mkdtempSync(join(tmp, "member-plugin-"));
    const pluginFile = join(pluginDir, "index.mjs");

    // Plugin collects every MemberExpression's (computed, property) tuple
    // and reports it as a JSON string so the test can assert the exact
    // values without depending on the reporting span format.
    writeFileSync(
      pluginFile,
      `
import { defineCheck, Category, Severity } from ${JSON.stringify(sdkUrl)};

export default defineCheck({
  id: "MemberCoverage",
  category: Category.Warning,
  basePriority: 0,
  defaultSeverity: Severity.Low,
  explanation: "Collects MemberExpression tuples for test assertions.",
  run(file, ctx) {
    if (!file.ast) return;
    for (const m of file.ast.findAll("MemberExpression")) {
      ctx.report({
        message: JSON.stringify({ computed: m.computed, property: m.property ?? null }),
        span: m.span,
      });
    }
  },
});
`,
    );
    writeFileSync(
      join(pluginDir, "package.json"),
      JSON.stringify({ type: "module", main: "index.mjs" }),
    );

    // Hand-built wire matching the Rust serializer output for:
    //
    //   const a = Math.random();         // node 1 (static)
    //   const b = Math["random"]();      // node 2 (computed, string)
    //   const c = process["env"]["FOO"]; // node 3 outer, node 4 inner (chained)
    //   const d = Math[k]();             // node 5 (computed, dynamic)
    //
    // Spans are placeholders (line 1, col 1, byte 0–1) — the test only
    // inspects the member-shape extras, not the span coordinates.
    const S = { line: 1, column: 1, start_byte: 0, end_byte: 1 };
    const ast = {
      rootIdx: 0,
      nodes: [
        // 0: Program
        { kind: "Program", span: S, firstChild: 1, nextSibling: -1 },
        // 1: Math.random — static (computed=false, property="random")
        {
          kind: "MemberExpression",
          span: S,
          firstChild: 6,
          nextSibling: 2,
          objectIdx: 6,
          property: "random",
          computed: false,
        },
        // 2: Math["random"] — computed, string literal (computed=true, property="random")
        {
          kind: "MemberExpression",
          span: S,
          firstChild: 7,
          nextSibling: 3,
          objectIdx: 7,
          property: "random",
          computed: true,
        },
        // 3: process["env"]["FOO"] outer — (computed=true, property="FOO")
        {
          kind: "MemberExpression",
          span: S,
          firstChild: 4,
          nextSibling: 5,
          objectIdx: 4,
          property: "FOO",
          computed: true,
        },
        // 4: process["env"] inner — (computed=true, property="env")
        {
          kind: "MemberExpression",
          span: S,
          firstChild: 8,
          nextSibling: -1,
          objectIdx: 8,
          property: "env",
          computed: true,
        },
        // 5: Math[k] — computed, dynamic (computed=true, property=undefined→null on wire)
        {
          kind: "MemberExpression",
          span: S,
          firstChild: 9,
          nextSibling: -1,
          objectIdx: 9,
          property: null,
          computed: true,
        },
        // 6: IdentifierReference "Math" (object of node 1)
        { kind: "IdentifierReference", span: S, firstChild: -1, nextSibling: -1, name: "Math" },
        // 7: IdentifierReference "Math" (object of node 2)
        { kind: "IdentifierReference", span: S, firstChild: -1, nextSibling: -1, name: "Math" },
        // 8: IdentifierReference "process" (object of node 4)
        { kind: "IdentifierReference", span: S, firstChild: -1, nextSibling: -1, name: "process" },
        // 9: IdentifierReference "Math" (object of node 5)
        { kind: "IdentifierReference", span: S, firstChild: -1, nextSibling: -1, name: "Math" },
      ],
    };

    const { reports, errors } = await runAstFixture(pluginDir, tmp, join(tmp, "member.ts"), "x", ast);
    assert.equal(errors.length, 0, `expected no host errors, got: ${JSON.stringify(errors)}`);

    // Parse the reported tuples (reported in document order = node index order).
    const tuples = reports.map((r) => JSON.parse(r.message));
    assert.equal(tuples.length, 5, "expected 5 MemberExpression reports");

    // Case 1: Math.random — dot-form (static), always resolves.
    assert.deepEqual(
      tuples[0],
      { computed: false, property: "random" },
      "Math.random: computed=false, property='random'",
    );

    // Case 2: Math["random"] — computed, string-literal index, resolves to "random".
    assert.deepEqual(
      tuples[1],
      { computed: true, property: "random" },
      "Math['random']: computed=true, property='random'",
    );

    // Cases 3a/3b: process["env"]["FOO"] — outer first (node 3 precedes node 4
    // in the flat array document order used by findAll).
    assert.deepEqual(
      tuples[2],
      { computed: true, property: "FOO" },
      "process['env']['FOO'] outer: computed=true, property='FOO'",
    );
    assert.deepEqual(
      tuples[3],
      { computed: true, property: "env" },
      "process['env'] inner: computed=true, property='env'",
    );

    // Case 4: Math[k] — computed, runtime variable, stays undefined (null on wire).
    assert.deepEqual(
      tuples[4],
      { computed: true, property: null },
      "Math[k]: computed=true, property=null (dynamic — not statically determinable)",
    );
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test("walk honours Walk.Skip", async () => {
  const tmp = mkdtempSync(join(tmpdir(), "cofferdam-walk-test-"));
  try {
    const pluginDir = makeAstSmokePlugin(tmp);

    // Class containing a method (Function). visitFunction returns "skip"
    // so descent into the body is suppressed; nested CallExpressions
    // inside the method body should NOT trigger findAll-equivalent
    // reports via walk. (findAll is unaffected by Skip.)
    const text = 'class C { greet() { return frobnicate(); } }\n';
    const ast = {
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
    };

    const { reports } = await runAstFixture(pluginDir, tmp, join(tmp, "skip.ts"), text, ast);
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
