// Node test proving criterion 3 (CD-86): SeoDuplicateTitle's `outputMode:
// true` (CD-85) makes it eligible for `cofferdam verify --dist`, run
// against the SAME fixture directory used by `cofferdam check` (CD-86's
// key design point — one check satisfies both the cross-file corpus
// criterion and the output-mode criterion). Spawns the release binary
// (build it first: `cargo build --release -p cofferdam-cli`), mirroring
// how packages/check-sdk/tests/plugin-host.test.mjs spawns tooling.
//
// Run: node --test examples-plugins/seo/test/verify-dist.test.mjs
// (after `cargo build --release -p cofferdam-cli` and
// `npm run build` in this package)

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { existsSync } from "node:fs";

const ROOT = dirname(fileURLToPath(import.meta.url));
const PKG = resolve(ROOT, "..");
const REPO_ROOT = resolve(PKG, "..", "..");
const FIXTURE = join(PKG, "fixture");
const CONFIG = join(PKG, "cofferdam.toml");

const COFFERDAM_BIN =
  process.env.COFFERDAM_BIN ??
  join(REPO_ROOT, "target", "release", process.platform === "win32" ? "cofferdam.exe" : "cofferdam");

test("cofferdam verify --dist finds SeoDuplicateTitle in the same fixture cofferdam check uses", () => {
  if (!existsSync(COFFERDAM_BIN)) {
    throw new Error(
      `cofferdam binary not found at ${COFFERDAM_BIN}. Build it first: cargo build --release -p cofferdam-cli`,
    );
  }

  let raw;
  try {
    raw = execFileSync(
      COFFERDAM_BIN,
      ["verify", "--dist", FIXTURE, "--format", "json", "--pretty", "--config", CONFIG],
      { encoding: "utf8" },
    );
  } catch (e) {
    // `verify` exits non-zero when findings meet the fail-on gate — a
    // successful run for this test. Use the captured stdout.
    if (e.stdout) raw = e.stdout.toString();
    else throw e;
  }

  const doc = JSON.parse(raw);
  assert.equal(doc.origin, "build_output", "output-mode findings are labeled origin: build_output");

  const titleFindings = doc.findings.filter((f) => f.check_id === "Warning.SeoDuplicateTitle");
  assert.equal(titleFindings.length, 2, "one finding for the duplicate <title>, one for the duplicate canonical");

  const titleFinding = titleFindings.find((f) => f.message.includes("Widget Catalog"));
  assert.ok(titleFinding, "duplicate <title> \"Widget Catalog\" finding is present");
  assert.ok(titleFinding.file.endsWith("fixture/page-a.html"), titleFinding.file);
  assert.equal(titleFinding.related.length, 1);
  assert.ok(titleFinding.related[0].file.endsWith("fixture/page-b.html"), titleFinding.related[0].file);

  const canonicalFinding = titleFindings.find((f) => f.message.includes("canonical URL"));
  assert.ok(canonicalFinding, "duplicate canonical URL finding is present");

  // page-c.html has a unique <title>/canonical — must not appear anywhere.
  for (const f of doc.findings) {
    assert.ok(
      !f.file.includes("page-c.html") && !(f.related ?? []).some((r) => r.file.includes("page-c.html")),
      "page-c.html (unique title/canonical) never appears in a SeoDuplicateTitle finding",
    );
  }

  // verify --dist only discovers .html/.htm — the .tsx fixture files
  // (page.tsx under no-metadata/, missing-alt/, empty-description/,
  // good/) must never appear as a finding's file.
  for (const f of doc.findings) {
    assert.ok(!f.file.endsWith(".tsx"), `no .tsx file should appear in verify --dist output, got ${f.file}`);
  }
});
