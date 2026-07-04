#!/usr/bin/env node
// scripts/test-plugin-sdk-major.mjs — cd-b1q acceptance test.
//
// Builds a synthetic plugin tree where node_modules/@cofferdam/check-sdk
// claims a future-major SDK version, drives the plugin host directly
// with a streamed header/end manifest (no files), and asserts the host
// emits a load_failed error mentioning "incompatible". This proves SDK
// major-version rejection lands at load time, not deep inside run().

import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const HOST = join(REPO_ROOT, "crates", "cofferdam-cli", "scripts", "plugin-host.mjs");

const work = mkdtempSync(join(tmpdir(), "cofferdam-sdk-major-"));
try {
  // Synthetic plugin entry that imports nothing — the host should reject
  // BEFORE attempting the dynamic import, so the contents don't matter
  // for major-mismatch detection.
  const pluginDir = join(work, "fake-plugin");
  mkdirSync(join(pluginDir, "node_modules", "@cofferdam", "check-sdk"), {
    recursive: true,
  });
  writeFileSync(
    join(pluginDir, "package.json"),
    JSON.stringify({
      name: "fake-plugin",
      type: "module",
      main: "index.mjs",
      dependencies: { "@cofferdam/check-sdk": "*" },
    }),
  );
  writeFileSync(join(pluginDir, "index.mjs"), "export default { id: 'X', run() {} };\n");
  writeFileSync(
    join(pluginDir, "node_modules", "@cofferdam", "check-sdk", "package.json"),
    JSON.stringify({ name: "@cofferdam/check-sdk", version: "99.0.0" }),
  );

  const header = { type: "header", wireVersion: 2, cwd: work, plugins: [pluginDir], options: {} };
  const input = `${JSON.stringify(header)}\n${JSON.stringify({ type: "end" })}\n`;

  const out = execFileSync("node", [HOST], {
    input,
    encoding: "utf8",
  });

  const records = out
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => JSON.parse(l));
  const errors = records.filter((r) => r.type === "error");
  const failure = errors.find((e) => e.kind === "load_failed");
  if (!failure) {
    console.error("FAIL: expected load_failed error, got response:");
    console.error(out);
    process.exit(1);
  }
  if (!/incompatible/i.test(failure.message) || !failure.message.includes("99.0.0")) {
    console.error(
      "FAIL: load_failed message did not mention major-mismatch / 99.0.0:",
      failure.message,
    );
    process.exit(1);
  }
  console.log("OK: incompatible SDK major rejected with named error");
  console.log("    " + failure.message);
} finally {
  rmSync(work, { recursive: true, force: true });
}
