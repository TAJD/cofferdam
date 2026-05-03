#!/usr/bin/env node
// scripts/check-spans.mjs — span round-trip helper for plugin-fixture CI.
//
// For each issue in cofferdam's --format json output, slice the source file
// by [byte_start, byte_end) and assert the slice equals an expected trigger
// (literal string or /regex/). Exits 0 on success, 1 on any mismatch.
//
// Usage:
//   node scripts/check-spans.mjs <issues.json> <source.ts> <trigger>
//
// <trigger> forms:
//   - Literal:  Rovikore               (slice must equal exactly)
//   - Regex:    /^https?:\/\//         (slice must match)
//
// Filed as cd-n14, child of cd-7e4. Reused by every plugin-fixture CI step.

import { readFileSync } from "node:fs";
import { argv, exit } from "node:process";

function usage() {
  console.error("Usage: check-spans.mjs <issues.json> <source.ts> <trigger>");
  console.error("  trigger: literal string, or /pattern/ for a regex");
  exit(2);
}

if (argv.length !== 5) usage();
const [, , issuesPath, sourcePath, triggerArg] = argv;

let issuesDoc;
try {
  issuesDoc = JSON.parse(readFileSync(issuesPath, "utf8"));
} catch (e) {
  console.error(`check-spans: could not read/parse ${issuesPath}: ${e.message}`);
  exit(1);
}

let source;
try {
  source = readFileSync(sourcePath); // Buffer — byte-indexed slicing.
} catch (e) {
  console.error(`check-spans: could not read ${sourcePath}: ${e.message}`);
  exit(1);
}

const issues = Array.isArray(issuesDoc) ? issuesDoc : issuesDoc.issues ?? [];
if (!Array.isArray(issues)) {
  console.error("check-spans: expected array `issues` or top-level array");
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

for (const [i, issue] of issues.entries()) {
  // Filter by file when an issue references a different source — keep the
  // tool reusable for multi-file fixtures later.
  if (issue.file && !sourcePath.endsWith(issue.file.replace(/^\.?\//, ""))) {
    continue;
  }

  const span = issue.span ?? issue;
  const { byte_start, byte_end } = span;
  if (typeof byte_start !== "number" || typeof byte_end !== "number") {
    console.error(`#${i}: missing byte_start/byte_end on span`, span);
    failures++;
    continue;
  }
  if (byte_start < 0 || byte_end > source.length || byte_start >= byte_end) {
    console.error(
      `#${i}: byte range [${byte_start}, ${byte_end}) out of bounds for source of length ${source.length}`,
    );
    failures++;
    continue;
  }

  const slice = source.slice(byte_start, byte_end).toString("utf8");
  const ok = triggerKind === "regex" ? trigger.test(slice) : slice === trigger;
  if (!ok) {
    console.error(
      `#${i}: span [${byte_start}, ${byte_end}) sliced to ${JSON.stringify(slice)}, ` +
        `expected ${triggerKind === "regex" ? trigger : JSON.stringify(trigger)}`,
    );
    failures++;
  }
}

if (failures > 0) {
  console.error(`check-spans: ${failures} of ${issues.length} issue spans failed round-trip`);
  exit(1);
}

console.log(`check-spans: ${issues.length} issue spans round-tripped OK`);
