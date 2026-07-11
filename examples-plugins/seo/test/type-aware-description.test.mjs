// Node test proving criterion 2 (CD-86): SeoNonEmptyDescription resolves
// `description` via `ctx.types.resolveLiteral` (CD-82) across files —
// `empty-description/page.tsx` imports `badDescription` (empty string,
// aliased to `description`) from `../constants.ts` and the check follows
// the import to flag it, not a same-file string match.
//
// `cofferdam`'s tsconfig.json discovery for type-aware *plugin* checks
// (`find_tsconfig` in crates/cofferdam-cli/src/main.rs) walks UP from the
// process's CWD only — it does not search into subdirectories of the
// directory `cofferdam check`/`verify` is pointed at. That's why this
// test spawns cofferdam with cwd = fixture/ (where fixture/tsconfig.json
// + fixture/../node_modules/ts-morph are discoverable), rather than from
// the repo root the way scripts/check-plugin-fixtures.mjs does — mirrors
// scripts/check-type-host-smoke.mjs's "Run from CI with cwd = the
// fixture directory" convention for the built-in ts-morph type host.
// Because of this CWD-anchored discovery, `examples-plugins/seo/
// expected.json` (generated via the repo-root-anchored plugin-fixture
// pipeline) legitimately does NOT show SeoNonEmptyDescription firing —
// see its Warning.PluginTypeHostUnavailable finding instead. This test
// is the actual proof that the check + resolveLiteral wiring works.
//
// Run: node --test examples-plugins/seo/test/type-aware-description.test.mjs
// (after `cargo build --release -p cofferdam-cli`, `npm run build` here,
// and `npm install` in examples-plugins/seo for the ts-morph devDependency)

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

test("SeoNonEmptyDescription flags an empty description resolved cross-file via resolveLiteral", () => {
  if (!existsSync(COFFERDAM_BIN)) {
    throw new Error(
      `cofferdam binary not found at ${COFFERDAM_BIN}. Build it first: cargo build --release -p cofferdam-cli`,
    );
  }
  if (!existsSync(join(PKG, "node_modules", "ts-morph"))) {
    throw new Error(`ts-morph not installed under ${PKG}/node_modules. Run: npm install (in examples-plugins/seo)`);
  }

  let raw;
  try {
    raw = execFileSync(
      COFFERDAM_BIN,
      [
        "check",
        ".",
        "--format",
        "json",
        "--pretty",
        "--no-baseline",
        "--config",
        CONFIG,
        "--only",
        "Warning.SeoNonEmptyDescription",
      ],
      { encoding: "utf8", cwd: FIXTURE },
    );
  } catch (e) {
    if (e.stdout) raw = e.stdout.toString();
    else throw e;
  }

  const doc = JSON.parse(raw);
  const findings = doc.findings.filter((f) => f.id === "Warning.SeoNonEmptyDescription");
  assert.equal(findings.length, 1, `expected exactly 1 SeoNonEmptyDescription finding, got ${JSON.stringify(doc.findings)}`);
  assert.ok(findings[0].file.endsWith("empty-description/page.tsx"), findings[0].file);
  assert.match(findings[0].message, /resolves to an empty string/);
});
