#!/usr/bin/env node
// Smoke test for scripts/check-spans.mjs. No SDK dependency — runs standalone
// with Node's built-in test runner.
//
// Run: node --test scripts/check-spans.test.mjs

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import { writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT = fileURLToPath(new URL("./check-spans.mjs", import.meta.url));

function withTmp(fn) {
  const dir = mkdtempSync(join(tmpdir(), "cofferdam-check-spans-"));
  try {
    fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function run(args) {
  try {
    const out = execFileSync("node", [SCRIPT, ...args], { encoding: "utf8" });
    return { code: 0, stdout: out, stderr: "" };
  } catch (e) {
    return {
      code: e.status ?? 1,
      stdout: e.stdout?.toString() ?? "",
      stderr: e.stderr?.toString() ?? "",
    };
  }
}

test("passes when literal trigger round-trips (canonical RobotReport shape)", () => {
  withTmp((dir) => {
    const src = "const x = 'Examplco';\n"; // 'Examplco' is bytes 11..19
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, src);

    // Canonical cofferdam JSON formatter shape: { findings: [...], summary: {...} }
    // with start_byte / end_byte at top level of each finding.
    const doc = {
      findings: [
        {
          id: "BrandCasing",
          category: "warning",
          severity: "warning",
          file: "fixture.ts",
          line: 1,
          column: 12,
          start_byte: 11,
          end_byte: 19,
          message: 'Brand name must be "EXAMPLCO", not "Examplco".',
        },
      ],
      summary: { total: 1, by_category: { warning: 1 } },
    };
    const docPath = join(dir, "findings.json");
    writeFileSync(docPath, JSON.stringify(doc));

    const r = run([docPath, sourcePath, "Examplco"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
    assert.match(r.stdout, /1 finding span\(s\) round-tripped OK/);
  });
});

test("fails when slice does not match literal trigger", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, "const x = 'examplco';\n"); // lowercase

    const docPath = join(dir, "findings.json");
    writeFileSync(
      docPath,
      JSON.stringify({ findings: [{ start_byte: 11, end_byte: 19 }] }),
    );

    const r = run([docPath, sourcePath, "Examplco"]);
    assert.equal(r.code, 1);
    assert.match(r.stderr, /expected "Examplco"/);
  });
});

test("regex trigger matches", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    // "fetch('https://example.com');" — 'https://example.com' is bytes 7..26
    writeFileSync(sourcePath, "fetch('https://example.com');\n");

    const docPath = join(dir, "findings.json");
    writeFileSync(
      docPath,
      JSON.stringify({ findings: [{ start_byte: 7, end_byte: 26 }] }),
    );

    const r = run([docPath, sourcePath, "/^https:\\/\\//"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
  });
});

test("rejects out-of-bounds byte ranges", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, "short\n");

    const docPath = join(dir, "findings.json");
    writeFileSync(
      docPath,
      JSON.stringify({ findings: [{ start_byte: 0, end_byte: 999 }] }),
    );

    const r = run([docPath, sourcePath, "short"]);
    assert.equal(r.code, 1);
    assert.match(r.stderr, /out of bounds/);
  });
});

test("accepts top-level array form", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    // "abc Examplco xyz" — 'Examplco' is bytes 4..12
    writeFileSync(sourcePath, "abc Examplco xyz\n");

    const docPath = join(dir, "findings.json");
    writeFileSync(
      docPath,
      JSON.stringify([{ start_byte: 4, end_byte: 12 }]),
    );

    const r = run([docPath, sourcePath, "Examplco"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
  });
});

test("legacy issues+byte_start fallback still works", () => {
  // Older drafts of the helper expected `issues` + `byte_start/byte_end`.
  // We tolerate that for forward-compat with any in-flight CI fixtures.
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, "abc Examplco xyz\n");

    const docPath = join(dir, "findings.json");
    writeFileSync(
      docPath,
      JSON.stringify({ issues: [{ span: { byte_start: 4, byte_end: 12 } }] }),
    );

    const r = run([docPath, sourcePath, "Examplco"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
  });
});
