#!/usr/bin/env node
// scripts/platform-purity.mjs — cd-7ws CI guardrails.
//
// Runs four mechanical checks that prevent oxc / TypeScript coupling
// from leaking back into platform-layer crates after the cd-8wj +
// cd-jub split. Source: design/platform-extensibility.md, "CI
// guardrails" section.
//
// Each check fails loudly with a message pointing at the design doc
// when triggered. Tested by deliberately introducing a regression
// locally — see cd-7ws acceptance.
//
// Exit 0 if all four pass, 1 otherwise (CI surfaces this as a job
// failure). The platform-purity doc test (#4) lives in cofferdam-core
// itself and is exercised by `cargo test --workspace`; this script's
// #4 is a defensive re-run so a missing or skipped doc test still
// fails this job.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const REPO_ROOT = new URL("..", import.meta.url).pathname.replace(/^\/([A-Z]:)/, "$1");
const DESIGN_DOC = "design/platform-extensibility.md";

const failures = [];

function fail(check, msg) {
  failures.push({ check, msg });
  console.error(`✗ ${check}: ${msg}`);
}

function pass(check, msg) {
  console.log(`✓ ${check}: ${msg}`);
}

// ──── Guardrail 1: cofferdam-core has no oxc transitive dep ──────────

function checkNoOxcInCore() {
  const NAME = "1. cofferdam-core has no oxc transitive dep";
  // Workspace-wide metadata, then walk resolve.nodes starting at
  // cofferdam-core to collect just its transitive set. Naive
  // `--manifest-path crates/cofferdam-core` returns the workspace
  // graph (which obviously contains oxc) — it's not a per-package
  // filter, just a workspace selector.
  const out = execFileSync("cargo", ["metadata", "--format-version", "1"], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 32 * 1024 * 1024,
  });
  const meta = JSON.parse(out);
  // Map id ↔ name via `meta.packages` (the only consistent source —
  // resolve.nodes' id format varies between registry deps
  // (`registry+...#name@version`) and path deps where the dir name
  // matches the crate name (`path+file:///...#version`, no name in
  // fragment).
  const nameById = new Map();
  for (const pkg of meta.packages) {
    nameById.set(pkg.id, pkg.name);
  }
  const nodeById = new Map();
  for (const node of meta.resolve.nodes) {
    nodeById.set(node.id, node);
  }
  const idByName = new Map();
  for (const [id, name] of nameById) {
    idByName.set(name, nodeById.get(id));
  }
  const root = idByName.get("cofferdam-core");
  if (!root) {
    fail(NAME, "could not find cofferdam-core in cargo metadata");
    return;
  }
  // BFS over deps.
  const visited = new Set([root.id]);
  const queue = [root];
  const oxcSeen = new Set();
  while (queue.length > 0) {
    const node = queue.shift();
    for (const dep of node.deps) {
      const depPkgName = nameById.get(dep.pkg) ?? dep.name;
      if (depPkgName.startsWith("oxc_")) {
        oxcSeen.add(depPkgName);
      }
      const depNode = nodeById.get(dep.pkg);
      if (depNode && !visited.has(depNode.id)) {
        visited.add(depNode.id);
        queue.push(depNode);
      }
    }
  }
  if (oxcSeen.size > 0) {
    fail(
      NAME,
      `cofferdam-core's resolved dep tree contains oxc_* crates: ${[...oxcSeen]
        .sort()
        .join(", ")}.\n` +
        `  Per ${DESIGN_DOC}, the platform crate must stay language-agnostic.\n` +
        `  Move the oxc-bound code into cofferdam-ts (or another adapter crate).`,
    );
    return;
  }
  pass(NAME, "no oxc_* in cofferdam-core's transitive deps");
}

// ──── Guardrail 2: no `use oxc` outside the adapter ──────────────────

function checkNoOxcImportsOutsideAdapter() {
  const NAME = "2. no `use oxc_*` outside cofferdam-ts";
  // Allowed crates: cofferdam-ts owns oxc and is the only crate that
  // may `use oxc_*`. cofferdam-checks routes through cofferdam_ts::oxc_*
  // (which is `use cofferdam_ts::oxc_ast::...`, NOT `use oxc_ast::...`),
  // so it should also be clean of bare `use oxc_`.
  const platformCrates = [
    "crates/cofferdam-core",
    "crates/cofferdam-engine",
    "crates/cofferdam-formatters",
    "crates/cofferdam-cli",
    "crates/cofferdam-lsp",
    "crates/cofferdam-checks",
  ];
  const offenders = [];
  for (const dir of platformCrates) {
    const full = join(REPO_ROOT, dir);
    if (!existsSync(full)) continue;
    walkRust(full, (path, line, content) => {
      // Match `use oxc_X` at the start of a logical use path. Allow
      // `use cofferdam_ts::oxc_X` (the sanctioned re-export path).
      // The regex: `use oxc_` not preceded by `::`.
      if (/(^|[^:\w])use\s+oxc_/.test(content)) {
        offenders.push(`${path}:${line}: ${content.trim()}`);
      }
    });
  }
  if (offenders.length > 0) {
    fail(
      NAME,
      `${offenders.length} bare \`use oxc_*\` line(s) outside cofferdam-ts:\n` +
        offenders.map((o) => `  ${o}`).join("\n") +
        `\n  Route oxc references via \`use cofferdam_ts::oxc_*\`.\n` +
        `  See ${DESIGN_DOC}.`,
    );
    return;
  }
  pass(NAME, "no bare oxc imports outside cofferdam-ts");
}

function walkRust(dir, visit) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      // Skip `target/` and `.git/`.
      if (entry.name === "target" || entry.name === ".git" || entry.name === "node_modules") {
        continue;
      }
      walkRust(full, visit);
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      const text = readFileSync(full, "utf8");
      const rel = relPath(full);
      let lineNo = 0;
      for (const line of text.split("\n")) {
        lineNo += 1;
        visit(rel, lineNo, line);
      }
    }
  }
}

function relPath(full) {
  // REPO_ROOT may or may not have a trailing slash depending on URL
  // resolution; strip both and forward-slash the result.
  const root = REPO_ROOT.replace(/[\\/]$/, "");
  return full
    .slice(root.length)
    .replace(/^[\\/]+/, "")
    .replace(/\\/g, "/");
}

// ──── Guardrail 3: no `oxc` in @cofferdam/check-sdk dist + src ────────

function checkSdkPureOfOxc() {
  const NAME = "3. @cofferdam/check-sdk has no oxc references";
  const sdkPaths = [
    "packages/check-sdk/src",
    "packages/check-sdk/dist",
  ];
  const offenders = [];
  for (const dir of sdkPaths) {
    const full = join(REPO_ROOT, dir);
    if (!existsSync(full)) continue;
    walkText(full, (path, line, content) => {
      // Match `oxc_` (rust-style) or `oxc-` and bare `oxc` *as identifier*.
      // Comments mentioning the word "oxc" are fine — those are
      // architectural references, not type references. Filter to actual
      // imports / type references via a heuristic: line contains `import`
      // or `from` AND `oxc`, OR line has a TS type-position `oxc_`.
      const isImportLike = /\bimport\b|\bfrom\b/.test(content);
      const hasOxcId = /(^|[^a-zA-Z_])oxc[_-]/.test(content);
      if (isImportLike && hasOxcId) {
        offenders.push(`${path}:${line}: ${content.trim()}`);
      } else if (/^\s*(import|export)\b.*\boxc\b/.test(content)) {
        offenders.push(`${path}:${line}: ${content.trim()}`);
      }
    });
  }
  if (offenders.length > 0) {
    fail(
      NAME,
      `${offenders.length} oxc reference(s) in SDK src/dist:\n` +
        offenders.map((o) => `  ${o}`).join("\n") +
        `\n  Plugin SDK must expose only cofferdam-namespaced types.\n` +
        `  See ${DESIGN_DOC}, "Plugin SDK AST surface" section.`,
    );
    return;
  }
  pass(NAME, "no oxc imports or type references in SDK src/dist");
}

function walkText(dir, visit) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name === ".git") continue;
      walkText(full, visit);
    } else if (
      entry.isFile() &&
      (entry.name.endsWith(".ts") || entry.name.endsWith(".js") || entry.name.endsWith(".d.ts"))
    ) {
      const text = readFileSync(full, "utf8");
      const rel = relPath(full);
      let lineNo = 0;
      for (const line of text.split("\n")) {
        lineNo += 1;
        visit(rel, lineNo, line);
      }
    }
  }
}

// ──── Guardrail 4: platform-purity doc test in cofferdam-core ────────

function checkPlatformPurityDocTest() {
  const NAME = "4. platform-purity doc test in cofferdam-core";
  // The doc test lives in cofferdam-core/src/language.rs. Run it
  // explicitly so this script catches a missing / broken doc test
  // even if `cargo test --workspace` was skipped.
  try {
    execFileSync("cargo", ["test", "--doc", "-p", "cofferdam-core"], {
      cwd: REPO_ROOT,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (err) {
    fail(
      NAME,
      `\`cargo test --doc -p cofferdam-core\` failed.\n  ${err.message}\n` +
        `  The platform-purity doc test in cofferdam-core/src/language.rs\n` +
        `  is the canary for the cd-jub abstraction. See ${DESIGN_DOC}.`,
    );
    return;
  }
  pass(NAME, "doc test in cofferdam-core/src/language.rs builds and passes");
}

// ──── Run ────────────────────────────────────────────────────────────

console.log("cd-7ws platform-purity guardrails\n");
checkNoOxcInCore();
checkNoOxcImportsOutsideAdapter();
checkSdkPureOfOxc();
checkPlatformPurityDocTest();

console.log("");
if (failures.length > 0) {
  console.error(`✗ ${failures.length} guardrail(s) failed.`);
  console.error(`  See ${DESIGN_DOC} for the architectural rationale.`);
  process.exit(1);
}
console.log(`✓ all 4 platform-purity guardrails passed.`);
