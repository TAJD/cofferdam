// Node-level unit test for CD-92: `resolveTsMorph`'s ancestor-directory
// walk must advance to the parent dir on every non-success path (a
// missing/unparsable package.json, an unresolvable entry, or an entry
// file that doesn't exist on disk) — a bare `continue` inside the try
// block previously skipped the `dir = parent` advance, re-checking the
// same broken directory every iteration until the 32-depth budget ran
// out, without ever reaching a working install further up the tree.

import { test } from "node:test";
import { strict as assert } from "node:assert";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve, join } from "node:path";
import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";

const ROOT = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(ROOT, "..", "..", "..");
const CORE_SCRIPT = resolve(REPO_ROOT, "crates", "cofferdam-cli", "scripts", "type-host-core.mjs");
const { resolveTsMorph } = await import(pathToFileURL(CORE_SCRIPT).href);

function writeTsMorphPackage(nodeModulesDir, { entry = "index.js", writeEntryFile = true } = {}) {
  const pkgDir = join(nodeModulesDir, "ts-morph");
  mkdirSync(pkgDir, { recursive: true });
  writeFileSync(
    join(pkgDir, "package.json"),
    JSON.stringify({ name: "ts-morph", version: "9.9.9", main: entry }),
  );
  if (writeEntryFile) {
    writeFileSync(join(pkgDir, entry), "export default {};\n");
  }
}

test("resolveTsMorph climbs past a directory with a broken ts-morph install to find a working one higher up", () => {
  const root = mkdtempSync(join(tmpdir(), "cofferdam-resolve-tsmorph-"));
  try {
    // Working install two levels up.
    writeTsMorphPackage(join(root, "node_modules"));
    // Broken install (entry file declared but never written) directly in
    // the start dir — must not stall the walk.
    const startDir = join(root, "a", "b");
    mkdirSync(startDir, { recursive: true });
    writeTsMorphPackage(join(startDir, "node_modules"), { writeEntryFile: false });

    const resolved = resolveTsMorph(startDir);
    assert.ok(resolved, "expected resolveTsMorph to find the working install higher up the tree");
    assert.equal(
      resolved.entryPath,
      join(root, "node_modules", "ts-morph", "index.js"),
      "expected the ancestor's working install, not the broken one",
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("resolveTsMorph returns null when no ancestor has a working install", () => {
  const root = mkdtempSync(join(tmpdir(), "cofferdam-resolve-tsmorph-none-"));
  try {
    const startDir = join(root, "a", "b");
    mkdirSync(startDir, { recursive: true });
    assert.equal(resolveTsMorph(startDir), null);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
