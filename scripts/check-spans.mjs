#!/usr/bin/env node
// scripts/check-spans.mjs — span round-trip helper for plugin-fixture CI.
//
// For each finding in cofferdam's --format json output, slice the source
// file by [start_byte, end_byte) and assert the slice equals an expected
// trigger (literal string or /regex/). Exits 0 on success, 1 on any
// mismatch.
//
// Usage:
//   node scripts/check-spans.mjs <findings.json> <source.ts> <trigger> [check-id]
//
// <trigger> forms:
//   - Literal:  Rovikore               (slice must equal exactly)
//   - Regex:    /^https?:\/\//         (slice must match)
//
// [check-id] (optional): when present, only findings whose `id` exactly
// matches are span-checked. Lets a fixture with one plugin's findings
// coexist with built-in findings (OrphanExport, DeadExport, …) firing
// on the same source — only the plugin's spans get verified.
//
// Schema: matches cofferdam-formatters/src/json.rs::RobotReport.
// Document shape:
//   { "findings": [ { "id", "file", "line", "column",
//                     "start_byte", "end_byte", "message", ... } ],
//     "summary": { ... } }
//
// Filed as cd-n14, child of cd-7e4. Reused by every plugin-fixture CI step.

import { readFileSync } from "node:fs";
import { argv, exit } from "node:process";

function usage() {
  console.error("Usage: check-spans.mjs <issues.json> <source.ts> <trigger> [check-id]");
  console.error("  trigger: literal string, or /pattern/ for a regex");
  console.error("  check-id (optional): only verify findings whose `id` matches");
  exit(2);
}

if (argv.length !== 5 && argv.length !== 6) usage();
const [, , findingsPath, sourcePath, triggerArg, checkIdFilter] = argv;

let doc;
try {
  doc = JSON.parse(readFileSync(findingsPath, "utf8"));
} catch (e) {
  console.error(`check-spans: could not read/parse ${findingsPath}: ${e.message}`);
  exit(1);
}

let source;
try {
  source = readFileSync(sourcePath); // Buffer — byte-indexed slicing.
} catch (e) {
  console.error(`check-spans: could not read ${sourcePath}: ${e.message}`);
  exit(1);
}

// Accept the canonical RobotReport shape (`{ findings: [...] }`) and a few
// permissive fallbacks so the helper survives schema drift / legacy callers
// without a flag day:
//   - { findings: [...] }   ← cofferdam formatter (canonical)
//   - { issues:   [...] }   ← older drafts; tolerated
//   - [ ... ]               ← bare array (test fixtures)
const findings = Array.isArray(doc)
  ? doc
  : doc.findings ?? doc.issues ?? [];
if (!Array.isArray(findings)) {
  console.error("check-spans: expected `findings` array, `issues` array, or top-level array");
  exit(1);
}

let trigger;
let triggerKind;
if (triggerArg.startsWith("/") && triggerArg.lastIndexOf("/") > 0) {
  const last = triggerArg.lastIndexOf("/");
  const pattern = triggerArg.slice(1, last);
  const flags = triggerArg.slice(last + 1);
  trigger = new RegExp(pattern, flags);
  triggerKind = "regex";
} else {
  trigger = triggerArg;
  triggerKind = "literal";
}

let failures = 0;
let checked = 0;

for (const [i, finding] of findings.entries()) {
  // Filter by check-id when requested — lets a plugin's spans be
  // verified even when other built-in checks fire on the same fixture.
  if (checkIdFilter && finding.id !== checkIdFilter) continue;

  // Filter by file when a finding references a different source — keep the
  // tool reusable for multi-file fixtures later.
  if (finding.file) {
    const wantNorm = sourcePath.replace(/\\/g, "/");
    const haveNorm = finding.file.replace(/\\/g, "/").replace(/^\.?\//, "");
    if (!wantNorm.endsWith(haveNorm)) continue;
  }

  // Cofferdam canonical fields; legacy `byte_start`/`byte_end` and nested
  // `span: { ... }` accepted for forward-compat with older test fixtures.
  const span = finding.span ?? finding;
  const startByte = span.start_byte ?? span.byte_start;
  const endByte = span.end_byte ?? span.byte_end;
  if (typeof startByte !== "number" || typeof endByte !== "number") {
    console.error(`#${i}: missing start_byte/end_byte on finding`, span);
    failures++;
    continue;
  }
  if (startByte < 0 || endByte > source.length || startByte >= endByte) {
    console.error(
      `#${i}: byte range [${startByte}, ${endByte}) out of bounds for source of length ${source.length}`,
    );
    failures++;
    continue;
  }

  checked++;
  const slice = source.slice(startByte, endByte).toString("utf8");
  const ok = triggerKind === "regex" ? trigger.test(slice) : slice === trigger;
  if (!ok) {
    console.error(
      `#${i}: span [${startByte}, ${endByte}) sliced to ${JSON.stringify(slice)}, ` +
        `expected ${triggerKind === "regex" ? trigger : JSON.stringify(trigger)}`,
    );
    failures++;
  }
}

if (failures > 0) {
  console.error(`check-spans: ${failures} of ${checked} checked spans failed round-trip`);
  exit(1);
}

console.log(`check-spans: ${checked} finding span(s) round-tripped OK`);
