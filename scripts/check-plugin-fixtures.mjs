#!/usr/bin/env node
// scripts/check-plugin-fixtures.mjs — CI-side companion to
// regen-plugin-fixtures.mjs. Runs cofferdam against every fixture and
// asserts the actual output matches the committed `expected.json`.
//
// Exits 0 if every fixture matches, non-zero (with a unified diff) on
// the first mismatch. Used by the plugin-sdk-e2e CI workflow.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { exit } from "node:process";

const REPO_ROOT = resolve(import.meta.dirname ?? new URL(".", import.meta.url).pathname, "..");
const FIXTURES_DIR = join(REPO_ROOT, "examples-plugins");
const COFFERDAM_BIN = process.env.COFFERDAM_BIN ?? join(
  REPO_ROOT,
  "target",
  "release",
  process.platform === "win32" ? "cofferdam.exe" : "cofferdam",
);

if (!existsSync(COFFERDAM_BIN)) {
  console.error(`cofferdam binary not found at ${COFFERDAM_BIN}.`);
  exit(2);
}

if (!existsSync(FIXTURES_DIR)) {
  console.log("No examples-plugins/ — nothing to check.");
  exit(0);
}

let mismatches = 0;
let checked = 0;

for (const name of readdirSync(FIXTURES_DIR)) {
  const dir = join(FIXTURES_DIR, name);
  if (!statSync(dir).isDirectory()) continue;
  const fixture = join(dir, "fixture.ts");
  const expectedPath = join(dir, "expected.json");
  if (!existsSync(fixture) || !existsSync(expectedPath)) continue;

  let raw;
  try {
    raw = execFileSync(
      COFFERDAM_BIN,
      ["check", fixture, "--format", "json", "--pretty"],
      { encoding: "utf8" },
    );
  } catch (e) {
    if (e.stdout) raw = e.stdout.toString();
    else {
      console.error(`failed to run cofferdam against ${fixture}: ${e.message}`);
      exit(2);
    }
  }

  const actual = normalise(raw);
  const expected = readFileSync(expectedPath, "utf8");
  checked++;

  if (actual !== expected) {
    mismatches++;
    console.error(`MISMATCH: ${name}/expected.json out of date.`);
    console.error("  Run scripts/regen-plugin-fixtures.mjs to regenerate.");
    console.error("  Unified diff (expected → actual):");
    printUnifiedDiff(expected, actual);
  }
}

if (mismatches > 0) {
  console.error(`\n${mismatches} of ${checked} fixture(s) out of date.`);
  exit(1);
}
console.log(`OK: ${checked} fixture(s) match committed expected.json.`);

// ---- helpers --------------------------------------------------------

function normalise(raw) {
  const doc = JSON.parse(raw);
  for (const f of doc.findings ?? []) {
    f.file = toRepoRelative(f.file);
  }
  return JSON.stringify(doc, null, 2) + "\n";
}

function toRepoRelative(absPath) {
  const repoNorm = REPO_ROOT.replace(/\\/g, "/");
  const norm = absPath.replace(/\\/g, "/");
  if (norm.startsWith(repoNorm + "/")) return norm.slice(repoNorm.length + 1);
  if (norm === repoNorm) return ".";
  return norm;
}

function printUnifiedDiff(a, b) {
  const aLines = a.split("\n");
  const bLines = b.split("\n");
  const max = Math.max(aLines.length, bLines.length);
  for (let i = 0; i < max; i++) {
    if (aLines[i] !== bLines[i]) {
      if (aLines[i] !== undefined) console.error(`-${aLines[i]}`);
      if (bLines[i] !== undefined) console.error(`+${bLines[i]}`);
    }
  }
}
