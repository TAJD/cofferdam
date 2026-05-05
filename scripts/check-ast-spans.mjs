#!/usr/bin/env node
// scripts/check-ast-spans.mjs — span round-trip guardrail (cd-svf).
//
// Runs cofferdam check against a fixture with COFFERDAM_PLUGIN_HOST_DUMP_WIRE
// set. The host writes its full per-file manifest (including the AST wire)
// to the named file. This script then verifies, for every emitted node:
//
//   source.slice(node.span.start_byte, node.span.end_byte)
//
// is non-empty and within bounds. Per-kind sanity checks layer on top —
// CallExpression must contain `(`, ImportDeclaration must contain
// `import`, etc. Catches off-by-one and encoding bugs in the wire
// builder before plugin authors see broken byte ranges.
//
// Exit: 0 if every node round-trips, 1 on any failure.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, unlinkSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { exit } from "node:process";

const REPO_ROOT = resolve(import.meta.dirname ?? new URL(".", import.meta.url).pathname, "..");
const COFFERDAM_BIN =
  process.env.COFFERDAM_BIN ??
  join(REPO_ROOT, "target", "release", process.platform === "win32" ? "cofferdam.exe" : "cofferdam");

if (!existsSync(COFFERDAM_BIN)) {
  console.error(`cofferdam binary not found at ${COFFERDAM_BIN}.`);
  exit(2);
}

const fixturePath = process.argv[2] ?? "examples-plugins/brand-casing/fixture.ts";
const fixtureConfig = process.argv[3] ?? "examples-plugins/brand-casing/cofferdam.toml";

const dumpPath = join(tmpdir(), `cofferdam-wire-${process.pid}.json`);
try {
  execFileSync(
    COFFERDAM_BIN,
    ["check", fixturePath, "--format", "json", "--config", fixtureConfig],
    {
      encoding: "utf8",
      env: { ...process.env, COFFERDAM_PLUGIN_HOST_DUMP_WIRE: dumpPath },
      cwd: REPO_ROOT,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
} catch (_e) {
  // Cofferdam exits non-zero when findings exist — fine for our purposes.
  // We only need the wire dump to land on disk.
  if (!existsSync(dumpPath)) {
    console.error(`failed to run cofferdam: wire dump not produced at ${dumpPath}`);
    exit(2);
  }
}

if (!existsSync(dumpPath)) {
  console.error(`expected wire dump at ${dumpPath}, found nothing — host may not have been invoked.`);
  exit(2);
}

const dump = JSON.parse(readFileSync(dumpPath, "utf8"));
unlinkSync(dumpPath);

const SANITY = {
  Program: () => true, // covers the whole file
  CallExpression: (s) => s.includes("("),
  ImportDeclaration: (s) => /\bimport\b/.test(s),
  // Functions cover declarations, expressions, AND class methods —
  // methods elide the `function` keyword. Accept anything containing
  // `(` followed eventually by `{` (or `=>` for arrow-shaped function
  // expressions, which oxc also surfaces as Function in some cases).
  Function: (s) => /\(/.test(s) && (/\{/.test(s) || /=>/.test(s)),
  ArrowFunctionExpression: (s) => s.includes("=>"),
  Class: (s) => /\bclass\b/.test(s),
  ObjectExpression: (s) => s.startsWith("{"),
  MemberExpression: (s) => /[.\[]/.test(s),
  IdentifierReference: (s) => /^[\p{L}_$][\p{L}\p{N}_$]*$/u.test(s),
};

let failed = 0;
let checked = 0;
let kindCounts = {};

for (const file of dump) {
  // Spans are UTF-8 byte offsets; JS strings index by UTF-16 code unit.
  // Slice through Buffer to honour the byte-offset semantics.
  const buffer = Buffer.from(file.text, "utf8");
  const nodes = file.ast?.nodes ?? [];
  if (nodes.length === 0) continue;
  for (const n of nodes) {
    const { kind, span } = n;
    kindCounts[kind] = (kindCounts[kind] ?? 0) + 1;
    if (
      span.start_byte < 0 ||
      span.end_byte > buffer.length ||
      span.start_byte > span.end_byte
    ) {
      failed++;
      console.error(
        `BAD SPAN: ${file.path} ${kind} @ ${span.line}:${span.column} ` +
          `bytes=[${span.start_byte}, ${span.end_byte}) buf-len=${buffer.length}`,
      );
      continue;
    }
    const slice = buffer.slice(span.start_byte, span.end_byte).toString("utf8");
    if (slice.length === 0) {
      failed++;
      console.error(
        `EMPTY SLICE: ${file.path} ${kind} @ ${span.line}:${span.column} bytes=[${span.start_byte}, ${span.end_byte})`,
      );
      continue;
    }
    const sanity = SANITY[kind];
    if (sanity && !sanity(slice)) {
      failed++;
      console.error(
        `SANITY FAIL: ${file.path} ${kind} @ ${span.line}:${span.column} sliced=${JSON.stringify(slice.slice(0, 40))}`,
      );
      continue;
    }
    checked++;
  }
}

console.log(`check-ast-spans: ${checked} node(s) round-tripped OK across ${dump.length} file(s).`);
console.log(`  by kind: ${Object.entries(kindCounts).map(([k, v]) => `${k}=${v}`).join(", ")}`);
if (failed > 0) {
  console.error(`${failed} span(s) failed validation.`);
  exit(1);
}
exit(0);
