// Node-level test for plugin-host.mjs's ctx.types wiring (CD-81).
//
// Spawns the actual production host script (crates/cofferdam-cli/scripts/
// plugin-host.mjs) as a subprocess and drives it over the real NDJSON wire
// (design/sdk-ast-wire.md), the same protocol the Rust CLI uses. Runs the
// script directly from its source location — `type-host-core.mjs` sits
// right next to it there, so the relative `import "./type-host-core.mjs"`
// resolves without needing Rust's temp-dir materialisation step.
//
// The ts-morph-backed happy path is gated on COFFERDAM_TYPE_HOST_TS_MORPH_ROOT
// (same env var `type_host.rs`'s Rust tests use) pointing at a directory
// whose `node_modules` contains ts-morph — skips silently when unset so CI
// without ts-morph stays green. The no-ts-morph / no-tsconfig path needs no
// gating: it's the default "nothing installed" behaviour.

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

/** A plugin whose `run()` reports whether `ctx.types` was present, and if
 * so, whether the resolved type at the given span includes `null`. */
const TYPE_AWARE_PLUGIN = `
export default {
  id: "Test.TypeAware",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "reports ctx.types.typeAt results for CD-81 tests",
  requiresTypes: true,
  options: {},
  async run(file, ctx) {
    if (!ctx.types) {
      ctx.report({ message: "no-types", span: { start_byte: 0, end_byte: 1 } });
      return;
    }
    // byte 6 is 'x' in "const x: string | null = null;"
    const facts = await ctx.types.typeAt(6, 7);
    ctx.report({
      message: facts && facts.includesNull ? "includesNull=true" : "includesNull=false",
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
`;

test("ctx.types is undefined and the plugin runs fine when no tsconfig is supplied", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cofferdam-plugin-host-test-"));
  try {
    const pluginDir = join(dir, "plugin");
    mkdirSync(pluginDir, { recursive: true });
    writeFileSync(join(pluginDir, "index.mjs"), TYPE_AWARE_PLUGIN);
    const filePath = join(dir, "a.ts").replace(/\\/g, "/");

    const { lines } = await runHost([
      { type: "header", wireVersion: 2, cwd: dir.replace(/\\/g, "/"), plugins: [pluginDir.replace(/\\/g, "/")], options: {} },
      {
        type: "file",
        path: filePath,
        text: "const x: string | null = null;\n",
        lineViews: [],
        layer: null,
        ast: null,
      },
      { type: "end" },
    ]);

    const reports = lines.filter((l) => l.type === "report");
    assert.equal(reports.length, 1, `expected one report; got ${JSON.stringify(lines)}`);
    assert.equal(reports[0].message, "no-types", "ctx.types must be undefined with no tsconfig");

    const done = lines.find((l) => l.type === "done");
    assert.ok(done, "must see a done record");
    assert.ok(
      "typeHostUnavailable" in done,
      "done.typeHostUnavailable must be present when a check declared requiresTypes:true",
    );
    assert.notEqual(done.typeHostUnavailable, null, "reason should be set (no tsconfig)");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("ctx.types.typeAt resolves a nullable type end-to-end via ts-morph", async () => {
  const tsMorphRoot = process.env.COFFERDAM_TYPE_HOST_TS_MORPH_ROOT;
  if (!tsMorphRoot) {
    return; // not configured — skip, matches type_host.rs's Rust test gating
  }

  const dir = mkdtempSync(join(tmpdir(), "cofferdam-plugin-host-test-"));
  try {
    const pluginDir = join(dir, "plugin");
    mkdirSync(pluginDir, { recursive: true });
    writeFileSync(join(pluginDir, "index.mjs"), TYPE_AWARE_PLUGIN);

    const samplePath = join(dir, "sample.ts");
    writeFileSync(samplePath, "const x: string | null = null;\nexport { x };\n");
    const tsconfigPath = join(dir, "tsconfig.json");
    writeFileSync(
      tsconfigPath,
      JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, include: ["sample.ts"] }),
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
        path: samplePath.replace(/\\/g, "/"),
        text: "const x: string | null = null;\nexport { x };\n",
        lineViews: [],
        layer: null,
        ast: null,
      },
      { type: "end" },
    ]);

    const reports = lines.filter((l) => l.type === "report");
    assert.equal(reports.length, 1, `expected one report; got ${JSON.stringify(lines)}\nstderr=${stderr}`);
    assert.equal(reports[0].message, "includesNull=true", `stderr=${stderr}`);

    const done = lines.find((l) => l.type === "done");
    assert.equal(done.typeHostUnavailable, null, `expected ts-morph to resolve fine; stderr=${stderr}`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
