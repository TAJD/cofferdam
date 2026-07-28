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
  // Fixture target is `fixture.ts` (single file) or `fixture/`
  // (directory of files for cross-file plugin checks — cd-9hp.6).
  const fixtureFile = join(dir, "fixture.ts");
  const fixtureDir = join(dir, "fixture");
  let fixture = null;
  if (existsSync(fixtureFile)) {
    fixture = fixtureFile;
  } else if (existsSync(fixtureDir) && statSync(fixtureDir).isDirectory()) {
    fixture = fixtureDir;
  }
  const expectedPath = join(dir, "expected.json");
  if (!fixture || !existsSync(expectedPath)) continue;

  // Match regen-plugin-fixtures.mjs: per-fixture cofferdam.toml, if
  // present, is passed via --config. Discovery starts from CWD (the
  // repo root in CI), not from the fixture's directory, so explicit
  // pointing is required.
  //
  // `--no-baseline` insulates these goldens from any baseline that may
  // exist under the repo (e.g. `.cofferdam/baseline.json` added by the
  // SDK dogfood — cd-sv9m). Plugin fixtures assert SDK behavior, not
  // baseline behavior; the auto-discovered baseline would otherwise
  // toggle the per-finding `baselined` field and the summary
  // `new`/`baselined` counts in the output, breaking every fixture.
  const fixtureConfig = join(dir, "cofferdam.toml");
  const args = ["check", fixture, "--format", "json", "--pretty", "--no-baseline"];
  if (existsSync(fixtureConfig)) args.push("--config", fixtureConfig);

  let raw;
  try {
    raw = execFileSync(
      COFFERDAM_BIN,
      args,
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
    if (Array.isArray(f.related)) {
      for (const r of f.related) {
        if (typeof r.file === "string") r.file = toRepoRelative(r.file);
      }
    }
    if (typeof f.message === "string") f.message = stripRepoRoot(f.message);
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

// Some findings (e.g. type-host-unavailable warnings) embed an absolute
// path in free-text `message` rather than a dedicated `file` field —
// `toRepoRelative` doesn't cover those. Strip the repo-root prefix
// wherever it appears in the message text so goldens stay portable
// across machines/CI runners instead of baking in a host-specific path.
function stripRepoRoot(message) {
  const repoNorm = REPO_ROOT.replace(/\\/g, "/");
  return message.replace(/\\/g, "/").split(repoNorm + "/").join("");
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
