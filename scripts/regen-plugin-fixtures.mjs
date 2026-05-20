#!/usr/bin/env node
// scripts/regen-plugin-fixtures.mjs — regenerate `expected.json` for every
// plugin fixture under `examples-plugins/<name>/`. Filed as cd-48g.
//
// Usage:
//   node scripts/regen-plugin-fixtures.mjs
//
// For each fixture directory the script:
//   1. Locates a `fixture.ts` (the input file the plugin is run against).
//   2. Runs `cofferdam check fixture.ts --format json --pretty` against
//      it. The cofferdam binary must already be built (`cargo build
//      --release -p cofferdam-cli`); we don't trigger the cargo build
//      from inside the script — that's the controller's job.
//   3. Normalises the output (forward-slash paths, drop machine-specific
//      noise) and writes it to `expected.json`.
//
// Convention (cd-48g): changes to `expected.json` larger than ±1 issue
// must be paired with a linked bead in the same commit. The diff
// summary printed at the end calls these out so reviewers don't miss
// them.
//
// CI usage: this script regenerates locally; CI runs the same
// `cofferdam check ...` invocation and `diff -u` against the
// committed `expected.json`. A diff failure means the author forgot
// to regenerate (or the fixture's behaviour changed unexpectedly).

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve, sep } from "node:path";
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
  console.error(`Build it first: cargo build --release -p cofferdam-cli`);
  console.error(`Or set COFFERDAM_BIN to point at an existing build.`);
  exit(2);
}

if (!existsSync(FIXTURES_DIR)) {
  console.log(`No examples-plugins/ directory found at ${FIXTURES_DIR} — nothing to regenerate.`);
  exit(0);
}

let regenerated = 0;
let bigDiffs = [];

for (const name of readdirSync(FIXTURES_DIR)) {
  const dir = join(FIXTURES_DIR, name);
  if (!statSync(dir).isDirectory()) continue;
  // Fixture target is either `fixture.ts` (single file) or `fixture/`
  // (a directory of files, for cross-file plugin checks — cd-9hp.6).
  const fixtureFile = join(dir, "fixture.ts");
  const fixtureDir = join(dir, "fixture");
  let fixture = null;
  if (existsSync(fixtureFile)) {
    fixture = fixtureFile;
  } else if (existsSync(fixtureDir) && statSync(fixtureDir).isDirectory()) {
    fixture = fixtureDir;
  }
  if (!fixture) continue;
  const expectedPath = join(dir, "expected.json");

  // Per-fixture cofferdam.toml lives next to the fixture so each
  // example is self-contained (plugin paths resolve relative to the
  // config dir, no repo-root config pollution). Pass it explicitly —
  // the engine's walk-up discovery starts from CWD, not the input
  // file, and won't find a sibling-of-fixture config on its own.
  const fixtureConfig = join(dir, "cofferdam.toml");
  const args = ["check", fixture, "--format", "json", "--pretty"];
  if (existsSync(fixtureConfig)) args.push("--config", fixtureConfig);

  let actual;
  try {
    const raw = execFileSync(
      COFFERDAM_BIN,
      args,
      { encoding: "utf8" },
    );
    actual = normalise(raw);
  } catch (e) {
    // cofferdam exits non-zero when findings exist — that's a
    // *successful* run for the fixture. Use the captured stdout.
    if (e.stdout) {
      actual = normalise(e.stdout.toString());
    } else {
      console.error(`failed to run cofferdam against ${fixture}: ${e.message}`);
      exit(2);
    }
  }

  const before = existsSync(expectedPath) ? JSON.parse(readFileSync(expectedPath, "utf8")) : null;
  const beforeCount = before?.findings?.length ?? null;
  const afterDoc = JSON.parse(actual);
  const afterCount = afterDoc.findings?.length ?? 0;

  if (before === null) {
    console.log(`${name}: created expected.json with ${afterCount} finding(s)`);
  } else if (beforeCount === afterCount) {
    console.log(`${name}: ${afterCount} finding(s) (unchanged count)`);
  } else {
    const delta = afterCount - beforeCount;
    const tag = Math.abs(delta) > 1 ? " — LARGE DIFF, link a bead in the commit" : "";
    console.log(`${name}: ${beforeCount} → ${afterCount} finding(s) (Δ${delta > 0 ? "+" : ""}${delta})${tag}`);
    if (Math.abs(delta) > 1) bigDiffs.push({ name, before: beforeCount, after: afterCount });
  }

  writeFileSync(expectedPath, actual);
  regenerated++;
}

console.log(`\nregenerated ${regenerated} expected.json file(s).`);
if (bigDiffs.length > 0) {
  console.log("\n*** Convention check: the following fixtures changed by more than ±1 finding:");
  for (const d of bigDiffs) {
    console.log(`  - ${d.name}: ${d.before} → ${d.after}`);
  }
  console.log("Each should be paired with a linked bead in the commit.");
}

// ---- helpers --------------------------------------------------------

function normalise(raw) {
  // The cofferdam JSON formatter already forward-slashes paths and emits
  // stable field ordering (BTreeMap). Round-trip through JSON.parse so
  // the file ends with a single trailing newline and no host-specific
  // line endings.
  const doc = JSON.parse(raw);
  // Make `file` paths repo-relative so the golden is portable across
  // workstations and CI runners. Applies to the primary span's `file`
  // and to every entry under `related[].file` (cd-9hp.6).
  for (const f of doc.findings ?? []) {
    f.file = toRepoRelative(f.file);
    if (Array.isArray(f.related)) {
      for (const r of f.related) {
        if (typeof r.file === "string") r.file = toRepoRelative(r.file);
      }
    }
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
