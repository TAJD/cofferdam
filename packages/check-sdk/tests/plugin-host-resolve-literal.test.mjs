// Node-level test for plugin-host.mjs's ctx.types.resolveLiteral wiring (CD-82).
//
// Spawns the actual production host script (crates/cofferdam-cli/scripts/
// plugin-host.mjs) as a subprocess and drives it over the real NDJSON wire
// (design/sdk-ast-wire.md), the same protocol the Rust CLI uses. Runs the
// script directly from its source location — `type-host-core.mjs` sits
// right next to it there, so the relative `import "./type-host-core.mjs"`
// resolves without needing Rust's temp-dir materialisation step.
//
// Gated on COFFERDAM_TYPE_HOST_TS_MORPH_ROOT (same env var `type_host.rs`'s
// Rust tests and plugin-host-types.test.mjs use) pointing at a directory
// whose `node_modules` contains ts-morph — skips silently when unset so CI
// without ts-morph stays green.

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";

const ROOT = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(ROOT, "..", "..", "..");
const HOST_SCRIPT = resolve(REPO_ROOT, "crates", "cofferdam-cli", "scripts", "plugin-host.mjs");

/** Drive the host script over stdin/stdout with the given NDJSON records. */
function runHost(records) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, [HOST_SCRIPT], { stdio: ["pipe", "pipe", "pipe"] });
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

/** A plugin whose `run()` resolves the `description` identifier imported
 * from constants.ts and reports its resolved literal value. */
const RESOLVE_LITERAL_PLUGIN = `
export default {
  id: "Test.ResolveLiteral",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "reports ctx.types.resolveLiteral results for CD-82 tests",
  requiresTypes: true,
  options: {},
  async run(file, ctx) {
    if (!ctx.types) {
      ctx.report({ message: "no-types", span: { start_byte: 0, end_byte: 1 } });
      return;
    }
    // "import { " is 9 bytes; "description" (11 chars) spans [9, 20).
    const facts = await ctx.types.resolveLiteral(9, 20);
    ctx.report({
      message: facts && facts.literalString !== undefined
        ? "literalString=" + facts.literalString
        : "unresolved",
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
`;

test("ctx.types.resolveLiteral resolves a cross-file string literal end-to-end via ts-morph", async () => {
  const tsMorphRoot = process.env.COFFERDAM_TYPE_HOST_TS_MORPH_ROOT;
  if (!tsMorphRoot) {
    return; // not configured — skip, matches type_host.rs's Rust test gating
  }

  const dir = mkdtempSync(join(tmpdir(), "cofferdam-plugin-host-resolve-literal-"));
  try {
    const pluginDir = join(dir, "plugin");
    mkdirSync(pluginDir, { recursive: true });
    writeFileSync(join(pluginDir, "index.mjs"), RESOLVE_LITERAL_PLUGIN);

    writeFileSync(
      join(dir, "constants.ts"),
      'export const description = "A great page about widgets";\n',
    );
    const pagePath = join(dir, "page.ts");
    writeFileSync(
      pagePath,
      'import { description } from "./constants";\nexport { description };\n',
    );
    const tsconfigPath = join(dir, "tsconfig.json");
    writeFileSync(
      tsconfigPath,
      JSON.stringify({
        compilerOptions: { strict: true, noEmit: true },
        include: ["constants.ts", "page.ts"],
      }),
    );

    const { lines, stderr } = await runHost([
      {
        type: "header",
        wireVersion: 2,
        // cwd is where the plugin host walks node_modules from to
        // resolve ts-morph (see type-host-core.mjs::resolveTsMorph).
        cwd: tsMorphRoot.replace(/\\/g, "/"),
        plugins: [pluginDir.replace(/\\/g, "/")],
        options: {},
        tsconfigPath: tsconfigPath.replace(/\\/g, "/"),
      },
      {
        type: "file",
        path: pagePath.replace(/\\/g, "/"),
        text: 'import { description } from "./constants";\nexport { description };\n',
        lineViews: [],
        layer: null,
        ast: null,
      },
      { type: "end" },
    ]);

    const reports = lines.filter((l) => l.type === "report");
    assert.equal(reports.length, 1, `expected one report; got ${JSON.stringify(lines)}\nstderr=${stderr}`);
    assert.equal(
      reports[0].message,
      "literalString=A great page about widgets",
      `stderr=${stderr}`,
    );

    const done = lines.find((l) => l.type === "done");
    assert.equal(done.typeHostUnavailable, null, `expected ts-morph to resolve fine; stderr=${stderr}`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("ctx.types is undefined and the resolveLiteral plugin runs fine when no tsconfig is supplied", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cofferdam-plugin-host-resolve-literal-notsconfig-"));
  try {
    const pluginDir = join(dir, "plugin");
    mkdirSync(pluginDir, { recursive: true });
    writeFileSync(join(pluginDir, "index.mjs"), RESOLVE_LITERAL_PLUGIN);
    const filePath = join(dir, "a.ts").replace(/\\/g, "/");

    const { lines } = await runHost([
      { type: "header", wireVersion: 2, cwd: dir.replace(/\\/g, "/"), plugins: [pluginDir.replace(/\\/g, "/")], options: {} },
      {
        type: "file",
        path: filePath,
        text: "export const a = 1;\n",
        lineViews: [],
        layer: null,
        ast: null,
      },
      { type: "end" },
    ]);

    const reports = lines.filter((l) => l.type === "report");
    assert.equal(reports.length, 1, `expected one report; got ${JSON.stringify(lines)}`);
    assert.equal(reports[0].message, "no-types", "ctx.types must be undefined with no tsconfig");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
