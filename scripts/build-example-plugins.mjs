#!/usr/bin/env node
// scripts/build-example-plugins.mjs — build every plugin under
// examples-plugins/ for local dev and CI (plugin-sdk-e2e.yml).
//
// Discovers plugins by directory (any examples-plugins/<name>/ with a
// package.json + tsconfig.json), so adding a new plugin needs no CI
// edit — CD-79's seo plugin was silently skipped by CI for a while
// because the workflow hardcoded a plugin name list that nobody
// remembered to update.
//
// Requires packages/check-sdk to already be built (pnpm build).
//
// Usage: node scripts/build-example-plugins.mjs

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { exit } from "node:process";

const REPO_ROOT = resolve(import.meta.dirname ?? new URL(".", import.meta.url).pathname, "..");
const PLUGINS_DIR = join(REPO_ROOT, "examples-plugins");
const SDK_DIR = join(REPO_ROOT, "packages", "check-sdk");
const TSC_BIN = join(SDK_DIR, "node_modules", ".bin", process.platform === "win32" ? "tsc.cmd" : "tsc");

if (!existsSync(join(SDK_DIR, "dist"))) {
  console.error(`${SDK_DIR}/dist not found — build the SDK first (pnpm build in packages/check-sdk).`);
  exit(2);
}

const plugins = readdirSync(PLUGINS_DIR).filter((name) => {
  const dir = join(PLUGINS_DIR, name);
  return statSync(dir).isDirectory() && existsSync(join(dir, "package.json")) && existsSync(join(dir, "tsconfig.json"));
});

for (const name of plugins) {
  const dir = join(PLUGINS_DIR, name);

  const sdkDest = join(dir, "node_modules", "@cofferdam", "check-sdk");
  mkdirSync(sdkDest, { recursive: true });
  cpSync(join(SDK_DIR, "package.json"), join(sdkDest, "package.json"));
  cpSync(join(SDK_DIR, "dist"), join(sdkDest, "dist"), { recursive: true });

  console.log(`building ${name}...`);
  execFileSync(TSC_BIN, ["-p", "."], { cwd: dir, stdio: "inherit", shell: process.platform === "win32" });
}

console.log(`OK: built ${plugins.length} plugin(s): ${plugins.join(", ")}`);
