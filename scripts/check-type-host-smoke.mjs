#!/usr/bin/env node
// scripts/check-type-host-smoke.mjs — CI smoke test for the ts-morph type
// host (cd-9hp.2.4).
//
// Exercises the type-host worker pool end-to-end against a committed
// fixture project (examples-type-host/unused-null) that has a
// tsconfig.json + ts-morph installed. Three assertions:
//
//   1. ENABLED: `cofferdam check` with type-aware checks on resolves real
//      TypeScript types through the live Node worker and flags every
//      redundant null guard in the fixture — including a CROSS-FILE case
//      whose operand type comes from an imported interface, which only
//      fires if project-wide resolution works.
//   2. OPT-OUT: the same run with `[engine] type_aware = false` produces
//      ZERO Warning.UnusedNullCheck findings — the escape hatch for CI
//      machines without a Node runtime.
//   3. COLD-START: `cofferdam type-host --ping` reports a project-init
//      time below a generous ceiling, pinning the worker against a
//      catastrophic startup regression (the number is logged either way).
//
// We use a committed fixture rather than an external repo (gistreact was
// the design-doc suggestion) so the run is hermetic and reproducible.
//
// Run from CI with cwd = the fixture directory, e.g.
//   (cd examples-type-host/unused-null && node ../../scripts/check-type-host-smoke.mjs)

import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { exit } from "node:process";

const REPO_ROOT = resolve(import.meta.dirname ?? new URL(".", import.meta.url).pathname, "..");
const FIXTURE_DIR = join(REPO_ROOT, "examples-type-host", "unused-null");
const COFFERDAM_BIN = process.env.COFFERDAM_BIN ?? join(
  REPO_ROOT,
  "target",
  "release",
  process.platform === "win32" ? "cofferdam.exe" : "cofferdam",
);

// Generous ceiling: a 2-file project inits in well under a second locally;
// a cold CI runner is slower but nowhere near this. The point is catching
// a 10x regression or a hang, not micro-benchmarking — so no flakes.
const PROJECT_INIT_CEILING_MS = 60_000;
const CHECK_ID = "Warning.UnusedNullCheck";
const EXPECTED_FLAGGED = 4; // 3 single-file + 1 cross-file (Widget.id)

if (!existsSync(COFFERDAM_BIN)) {
  console.error(`cofferdam binary not found at ${COFFERDAM_BIN}.`);
  console.error("Build it first: cargo build --release -p cofferdam-cli");
  exit(2);
}
if (!existsSync(join(FIXTURE_DIR, "tsconfig.json"))) {
  console.error(`fixture not found at ${FIXTURE_DIR}.`);
  exit(2);
}
if (!existsSync(join(FIXTURE_DIR, "node_modules", "ts-morph"))) {
  console.error(`ts-morph is not installed in ${FIXTURE_DIR}.`);
  console.error("Install it first: (cd examples-type-host/unused-null && npm install)");
  exit(2);
}

let failures = 0;

// --- 1. ENABLED: type-aware checks resolve types and flag the fixture ---
{
  const findings = runCheck(["check", "src", "--format", "json", "--no-config", "--no-baseline", "--no-cache"]);
  const flagged = findings.filter((f) => f.id === CHECK_ID);
  if (flagged.length === EXPECTED_FLAGGED) {
    console.log(`OK: type host resolved types — ${flagged.length} ${CHECK_ID} finding(s), incl. the cross-file case.`);
  } else {
    failures++;
    console.error(`FAIL: expected ${EXPECTED_FLAGGED} ${CHECK_ID} finding(s) with the type host enabled, got ${flagged.length}.`);
    console.error("  A short count usually means project-wide (cross-file) type resolution did not run.");
    for (const f of flagged) console.error(`    - ${f.file}:${f.line} ${f.message}`);
  }
}

// --- 2. OPT-OUT: [engine] type_aware = false disables the check ---------
{
  const findings = runCheck(["check", "src", "--format", "json", "--config", "cofferdam.opt-out.toml", "--no-baseline", "--no-cache"]);
  const flagged = findings.filter((f) => f.id === CHECK_ID);
  if (flagged.length === 0) {
    console.log("OK: `[engine] type_aware = false` disabled the type-aware check (0 findings).");
  } else {
    failures++;
    console.error(`FAIL: opt-out config should yield 0 ${CHECK_ID} findings, got ${flagged.length}.`);
    for (const f of flagged) console.error(`    - ${f.file}:${f.line} ${f.message}`);
  }
}

// --- 3. COLD-START: project-init time is measured and bounded -----------
{
  const tsconfig = join(FIXTURE_DIR, "tsconfig.json");
  const out = run(COFFERDAM_BIN, [
    "type-host", "--ping", "--robot",
    "--project", FIXTURE_DIR,
    "--tsconfig", tsconfig,
  ], REPO_ROOT);
  let ping;
  try {
    ping = JSON.parse(out);
  } catch (e) {
    failures++;
    console.error(`FAIL: type-host --ping did not return JSON: ${e.message}\n${out}`);
    ping = null;
  }
  if (ping) {
    if (!ping.ok) {
      failures++;
      console.error(`FAIL: type-host --ping reported an error: ${ping.error}`);
    } else {
      const initMs = ping.timings?.projectInitMs;
      console.log(`MEASURED: ts-morph ${ping.tsMorphVersion ?? "?"} — import ${ping.timings?.tsMorphImportMs ?? "?"} ms, project init ${initMs ?? "?"} ms, total ${ping.timings?.totalMs ?? "?"} ms.`);
      if (typeof initMs === "number" && initMs > PROJECT_INIT_CEILING_MS) {
        failures++;
        console.error(`FAIL: project init ${initMs} ms exceeds the ${PROJECT_INIT_CEILING_MS} ms ceiling.`);
      }
    }
  }
}

if (failures > 0) {
  console.error(`\n${failures} type-host smoke assertion(s) failed.`);
  exit(1);
}
console.log("\nAll type-host smoke assertions passed.");

// ---- helpers --------------------------------------------------------

// Run cofferdam from the fixture directory so tsconfig + config discovery
// resolve against the fixture, exactly as a real user invocation would.
function runCheck(args) {
  const raw = run(COFFERDAM_BIN, args, FIXTURE_DIR);
  let doc;
  try {
    doc = JSON.parse(raw);
  } catch (e) {
    console.error(`failed to parse cofferdam JSON output: ${e.message}\n${raw}`);
    exit(2);
  }
  return doc.findings ?? [];
}

// `cofferdam check` exits non-zero when findings gate the run; that's not
// a script error, so capture stdout from the thrown error too.
function run(bin, args, cwd) {
  try {
    return execFileSync(bin, args, { encoding: "utf8", cwd });
  } catch (e) {
    if (e.stdout) return e.stdout.toString();
    console.error(`failed to run ${bin} ${args.join(" ")}: ${e.message}`);
    exit(2);
  }
}
