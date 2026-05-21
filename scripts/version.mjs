#!/usr/bin/env node
// Deterministic version manager for cofferdam (cd-9hp.2 release tooling).
//
// cofferdam's version lives in SIX places (see .claude/skills/cut-release).
// Keeping them in lockstep by hand is the footgun that shipped v0.3.3 to
// npm with every in-repo file still reading 0.3.2 — silent, because
// release.yml derives the published version from the git tag and never
// checked the repo. This script makes both halves mechanical:
//
//   node scripts/version.mjs check [X.Y.Z]
//       Read all six locations and assert they agree. With an argument,
//       also assert they equal it (release.yml passes the tag here, so a
//       forgotten bump fails the release instead of shipping a lie).
//       Exit 0 on success, 1 on any mismatch.
//
//   node scripts/version.mjs set X.Y.Z
//       Rewrite the four human-edited files (Cargo.toml workspace version
//       + the cofferdam-* path-dep pins, both package.json) to X.Y.Z,
//       format-preserving and dependency-free (no cargo-edit). Cargo.lock
//       and docs/public/checks.json are GENERATED — run `cargo build
//       --workspace` and `cofferdam gen-docs` after (the script prints the
//       commands; `--regen` runs them for you).
//
// The version locations (NOT a fixed six — derived, because they drift):
//   1. Cargo.toml          [workspace.package] version
//   2. ANY crate Cargo.toml   cofferdam-* path-dep pins. The root lists
//      most under [workspace.dependencies], but cofferdam-cli pins
//      cofferdam-lsp directly — so we scan every Cargo.toml, not just
//      the root. (This member-crate pin is the one the skill's
//      "six places" list missed and that stayed stale through v0.3.3.)
//   3. packages/check-sdk/package.json   "version"
//   4. packages/cofferdam/package.json   "version"
//   5. Cargo.lock          [[package]] cofferdam-* versions   (generated)
//   6. docs/public/checks.json   cofferdam_version            (generated)
//
// Deliberately NOT covered: editors/vscode/package.json. The VS Code
// extension is a stub published by no workflow and versions
// independently — see .claude/skills/cut-release. If it ever joins the
// lockstep release, add it here.

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { execFileSync } from "node:child_process";

const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const P = (...parts) => join(REPO_ROOT, ...parts);

const CARGO_TOML = P("Cargo.toml");
const CARGO_LOCK = P("Cargo.lock");
const CHECK_SDK_PKG = P("packages", "check-sdk", "package.json");
const COFFERDAM_PKG = P("packages", "cofferdam", "package.json");
const CHECKS_JSON = P("docs", "public", "checks.json");

// Every Cargo.toml that may carry a cofferdam-* path-dep pin: the
// workspace root plus each member crate.
function cargoTomlFiles() {
  const files = [CARGO_TOML];
  const cratesDir = P("crates");
  let entries = [];
  try {
    entries = readdirSync(cratesDir, { withFileTypes: true });
  } catch {
    entries = [];
  }
  for (const e of entries) {
    if (e.isDirectory()) files.push(join(cratesDir, e.name, "Cargo.toml"));
  }
  return files;
}

// A cofferdam-* path-dep pin line, in any section of any Cargo.toml:
//   cofferdam-core = { path = "crates/cofferdam-core", version = "0.3.4" }
//   cofferdam-lsp = { path = "../cofferdam-lsp", version = "0.3.4" }
const PIN_RE =
  /^(\s*cofferdam-[a-z]+\s*=\s*\{\s*path\s*=\s*"[^"]*",\s*version\s*=\s*")([^"]+)(".*)$/;
const PIN_CAPTURE_RE =
  /^\s*(cofferdam-[a-z]+)\s*=\s*\{\s*path\s*=\s*"[^"]*",\s*version\s*=\s*"([^"]+)"/;

const SEMVER_RE = /^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$/;

function main() {
  const [cmd, arg] = process.argv.slice(2);
  if (cmd === "check") {
    process.exit(runCheck(arg));
  } else if (cmd === "set") {
    process.exit(runSet(arg));
  } else {
    process.stderr.write(
      "usage:\n" +
        "  node scripts/version.mjs check [X.Y.Z]\n" +
        "  node scripts/version.mjs set X.Y.Z [--regen]\n",
    );
    process.exit(2);
  }
}

// --- check ------------------------------------------------------------

function runCheck(expected) {
  if (expected) {
    expected = expected.replace(/^v/, "");
    if (!SEMVER_RE.test(expected)) {
      fail(`expected version '${expected}' is not semver-shaped`);
      return 1;
    }
  }
  const found = collectVersions();

  // Print a table of every location and its version.
  const width = Math.max(...found.map((f) => f.label.length));
  for (const f of found) {
    process.stdout.write(`  ${f.label.padEnd(width)}  ${f.version ?? "<missing>"}\n`);
  }

  const distinct = new Set(found.map((f) => f.version));
  const missing = found.filter((f) => f.version == null);

  let ok = true;
  if (missing.length > 0) {
    fail(`could not read version from: ${missing.map((m) => m.label).join(", ")}`);
    ok = false;
  }
  if (distinct.size > 1) {
    fail(`version locations disagree: ${[...distinct].filter(Boolean).join(", ")}`);
    ok = false;
  }
  if (expected && !(distinct.size === 1 && distinct.has(expected))) {
    fail(`in-repo version ${[...distinct].join("/")} does not match expected ${expected}`);
    ok = false;
  }

  if (ok) {
    const v = [...distinct][0];
    process.stdout.write(
      `\nOK: all ${found.length} locations at ${v}${expected ? " (matches tag)" : ""}\n`,
    );
    return 0;
  }
  return 1;
}

function collectVersions() {
  const out = [];

  out.push({
    label: "Cargo.toml [workspace.package]",
    version: workspacePackageVersion(readFileSync(CARGO_TOML, "utf8")),
  });

  // cofferdam-* path-dep pins from every Cargo.toml.
  for (const file of cargoTomlFiles()) {
    const rel = file.slice(REPO_ROOT.length + 1).replace(/\\/g, "/");
    for (const pin of cargoPins(readFileSync(file, "utf8"))) {
      out.push({ label: `${rel} pin ${pin.crate}`, version: pin.version });
    }
  }

  out.push({ label: "packages/check-sdk", version: pkgJsonVersion(CHECK_SDK_PKG) });
  out.push({ label: "packages/cofferdam", version: pkgJsonVersion(COFFERDAM_PKG) });

  for (const pkg of lockfileCofferdamVersions(readFileSync(CARGO_LOCK, "utf8"))) {
    out.push({ label: `Cargo.lock ${pkg.crate}`, version: pkg.version });
  }

  out.push({ label: "docs/public/checks.json", version: checksJsonVersion() });

  return out;
}

// --- set --------------------------------------------------------------

function runSet(version) {
  if (!version) {
    fail("set requires a version argument");
    return 2;
  }
  version = version.replace(/^v/, "");
  if (!SEMVER_RE.test(version)) {
    fail(`'${version}' is not semver-shaped (e.g. 0.3.4 or 0.3.4-rc1)`);
    return 2;
  }
  const regen = process.argv.includes("--regen");

  let pinCount = 0;
  for (const file of cargoTomlFiles()) {
    pinCount += setCargoToml(file, version, file === CARGO_TOML);
  }
  setPkgJson(CHECK_SDK_PKG, version);
  setPkgJson(COFFERDAM_PKG, version);
  process.stdout.write(
    `set workspace version + ${pinCount} cofferdam-* pin(s) + both package.json to ${version}\n`,
  );

  if (regen) {
    process.stdout.write("regenerating Cargo.lock (cargo build --workspace)...\n");
    execFileSync("cargo", ["build", "--workspace"], { cwd: REPO_ROOT, stdio: "inherit" });
    process.stdout.write("regenerating checks.json (cofferdam gen-docs)...\n");
    execFileSync("cargo", ["run", "-p", "cofferdam-cli", "--quiet", "--", "gen-docs"], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });
    process.stdout.write("\nDone. Verify with: node scripts/version.mjs check " + version + "\n");
  } else {
    process.stdout.write(
      "\nGenerated files still stale — run:\n" +
        "  cargo build --workspace                       # Cargo.lock\n" +
        "  cargo run -p cofferdam-cli -- gen-docs        # docs/public/checks.json\n" +
        "  node scripts/version.mjs check " +
        version +
        "\n",
    );
  }
  return 0;
}

// Bump one Cargo.toml: the [workspace.package] version (root only) and
// every cofferdam-* path-dep pin (any section). Returns the pin count.
function setCargoToml(file, version, isRoot) {
  const lines = readFileSync(file, "utf8").split("\n");
  let section = "";
  let bumpedWorkspace = false;
  let pins = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const header = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (header) {
      section = header[1];
      continue;
    }
    if (isRoot && section === "workspace.package" && /^version\s*=\s*"[^"]+"/.test(line)) {
      lines[i] = line.replace(/^(version\s*=\s*")[^"]+(")/, `$1${version}$2`);
      bumpedWorkspace = true;
      continue;
    }
    const m = line.match(PIN_RE);
    if (m) {
      lines[i] = `${m[1]}${version}${m[3]}`;
      pins++;
    }
  }
  if (isRoot && !bumpedWorkspace) {
    throw new Error("could not find [workspace.package] version in root Cargo.toml");
  }
  writeFileSync(file, lines.join("\n"));
  return pins;
}

function setPkgJson(path, version) {
  const text = readFileSync(path, "utf8");
  // Targeted replace of the top-level "version" key, format-preserving.
  // package.json uses dependency NAMES as keys, never a nested "version"
  // key, so the first (only) "version": occurrence is the package's own.
  const re = /("version"\s*:\s*)"[^"]+"/;
  if (!re.test(text)) {
    // Distinguish "key absent" (a real error) from "already at target"
    // (idempotent no-op) — a plain `next === text` check conflates them.
    throw new Error(`could not find "version" key in ${path}`);
  }
  writeFileSync(path, text.replace(re, `$1"${version}"`));
}

// --- parsers ----------------------------------------------------------

function workspacePackageVersion(cargo) {
  const lines = cargo.split("\n");
  let section = "";
  for (const line of lines) {
    const header = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (header) {
      section = header[1];
      continue;
    }
    if (section === "workspace.package") {
      const m = line.match(/^version\s*=\s*"([^"]+)"/);
      if (m) return m[1];
    }
  }
  return null;
}

function cargoPins(cargo) {
  const pins = [];
  for (const line of cargo.split("\n")) {
    const m = line.match(PIN_CAPTURE_RE);
    if (m) pins.push({ crate: m[1], version: m[2] });
  }
  return pins;
}

function pkgJsonVersion(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8")).version ?? null;
  } catch {
    return null;
  }
}

function lockfileCofferdamVersions(lock) {
  // Cargo.lock is a sequence of [[package]] blocks:
  //   [[package]]
  //   name = "cofferdam-core"
  //   version = "0.3.2"
  const out = [];
  const blocks = lock.split("[[package]]");
  for (const block of blocks) {
    const name = block.match(/^\s*name\s*=\s*"(cofferdam-[a-z]+)"/m);
    if (!name) continue;
    const ver = block.match(/^\s*version\s*=\s*"([^"]+)"/m);
    out.push({ crate: name[1], version: ver ? ver[1] : null });
  }
  out.sort((a, b) => a.crate.localeCompare(b.crate));
  return out;
}

function checksJsonVersion() {
  try {
    return JSON.parse(readFileSync(CHECKS_JSON, "utf8")).cofferdam_version ?? null;
  } catch {
    return null;
  }
}

function fail(msg) {
  process.stderr.write(`ERROR: ${msg}\n`);
}

main();
