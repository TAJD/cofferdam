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

test("passes when literal trigger round-trips", () => {
  withTmp((dir) => {
    const src = "const x = 'Rovikore';\n"; // 'Rovikore' is bytes 11..19
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, src);

    const issues = {
      issues: [
        { check_id: "BrandCasing", span: { byte_start: 11, byte_end: 19 } },
      ],
    };
    const issuesPath = join(dir, "issues.json");
    writeFileSync(issuesPath, JSON.stringify(issues));

    const r = run([issuesPath, sourcePath, "Rovikore"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
    assert.match(r.stdout, /1 issue spans round-tripped OK/);
  });
});

test("fails when slice does not match literal trigger", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, "const x = 'rovikore';\n"); // lowercase

    const issuesPath = join(dir, "issues.json");
    writeFileSync(
      issuesPath,
      JSON.stringify({ issues: [{ span: { byte_start: 11, byte_end: 19 } }] }),
    );

    const r = run([issuesPath, sourcePath, "Rovikore"]);
    assert.equal(r.code, 1);
    assert.match(r.stderr, /expected "Rovikore"/);
  });
});

test("regex trigger matches", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    // "fetch('https://example.com');" — 'https://example.com' is bytes 7..26
    writeFileSync(sourcePath, "fetch('https://example.com');\n");

    const issuesPath = join(dir, "issues.json");
    writeFileSync(
      issuesPath,
      JSON.stringify({ issues: [{ span: { byte_start: 7, byte_end: 26 } }] }),
    );

    const r = run([issuesPath, sourcePath, "/^https:\\/\\//"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
  });
});

test("rejects out-of-bounds byte ranges", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    writeFileSync(sourcePath, "short\n");

    const issuesPath = join(dir, "issues.json");
    writeFileSync(
      issuesPath,
      JSON.stringify({ issues: [{ span: { byte_start: 0, byte_end: 999 } }] }),
    );

    const r = run([issuesPath, sourcePath, "short"]);
    assert.equal(r.code, 1);
    assert.match(r.stderr, /out of bounds/);
  });
});

test("accepts top-level array form", () => {
  withTmp((dir) => {
    const sourcePath = join(dir, "fixture.ts");
    // "abc Rovikore xyz" — 'Rovikore' is bytes 4..12
    writeFileSync(sourcePath, "abc Rovikore xyz\n");

    const issuesPath = join(dir, "issues.json");
    writeFileSync(
      issuesPath,
      JSON.stringify([{ span: { byte_start: 4, byte_end: 12 } }]),
    );

    const r = run([issuesPath, sourcePath, "Rovikore"]);
    assert.equal(r.code, 0, `stderr: ${r.stderr}`);
  });
});
