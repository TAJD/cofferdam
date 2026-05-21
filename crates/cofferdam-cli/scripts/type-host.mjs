#!/usr/bin/env node
// cofferdam type host (cd-9hp.2 / cp1).
//
// Spawned by the cofferdam Rust CLI to expose TypeScript's type system
// (via ts-morph) to built-in checks declaring `requires_types = true`.
// Reads NDJSON requests from stdin, writes NDJSON responses to stdout.
//
// Wire shape spec: design/type-host-wire.md.
//
// cp1 implements only the `ping` method — enough to spawn the worker,
// dynamic-import ts-morph (optionally), open a Project (optionally),
// and report wall-clock timings. cp2 adds `resolveTypes` and persistent
// Project handles between requests.

import { readFileSync, statSync } from "node:fs";
import { createInterface } from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join as joinPath, resolve as resolvePath } from "node:path";

// State persisted across requests in this worker's lifetime. The
// ts-morph module is imported once and reused; Projects are cached by
// tsconfig path so the costly init (seconds on a large repo) is paid
// once per analysis run, not per query.
const state = {
  tsMorph: null, // null = not loaded, object = loaded module
  tsMorphVersion: null, // string | null
  tsMorphLoadError: null, // string | null
  projects: new Map(), // tsconfigPath -> ts-morph Project
};

const rl = createInterface({ input: process.stdin, terminal: false });

rl.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;

  let req;
  try {
    req = JSON.parse(trimmed);
  } catch (e) {
    writeResponse({
      id: "<unparseable>",
      ok: false,
      error: { code: "internal", message: `bad request JSON: ${e.message}` },
    });
    return;
  }

  const id = typeof req.id === "string" ? req.id : "<missing-id>";
  const method = req.method;
  const params = req.params ?? {};

  try {
    if (method === "ping") {
      const result = await handlePing(params);
      writeResponse({ id, ok: true, result });
    } else if (method === "openProject") {
      const result = await handleOpenProject(params);
      writeResponse({ id, ok: true, result });
    } else if (method === "typeAt") {
      const result = await handleTypeAt(params);
      writeResponse({ id, ok: true, result });
    } else {
      writeResponse({
        id,
        ok: false,
        error: { code: "method_unknown", message: `unknown method: ${method}` },
      });
    }
  } catch (e) {
    // Throw sites attach a `.code` property when the failure is a
    // structured wire error (`ts_morph_unavailable`, `project_init_failed`).
    // Anything else becomes `internal`.
    const code = (e && typeof e === "object" && typeof e.code === "string" && e.code) || "internal";
    writeResponse({
      id,
      ok: false,
      error: { code, message: e instanceof Error ? e.message : String(e) },
    });
  }
});

rl.on("close", () => {
  // Stdin closed; nothing in-flight (rl emits 'line' sync per chunk),
  // so it's safe to exit immediately.
  process.exit(0);
});

// --- methods ----------------------------------------------------------

async function handlePing(params) {
  const t0 = nowMs();
  const loadTsMorph = params.loadTsMorph !== false; // default true
  const openProject = params.openProject ?? null;

  let tsMorphImportMs = null;
  let projectInitMs = null;

  if (loadTsMorph) {
    tsMorphImportMs = await ensureTsMorphLoaded();
    if (state.tsMorphLoadError) {
      // Surface as a structured error to the caller. ts-morph not being
      // available is a normal failure mode for projects that haven't
      // installed it, not a worker bug.
      const err = new Error(state.tsMorphLoadError);
      err.code = "ts_morph_unavailable";
      throw err;
    }

    if (openProject && typeof openProject.tsconfigPath === "string") {
      const initStart = nowMs();
      try {
        // Construct a Project rooted at the supplied tsconfig. Heavy:
        // ts-morph reads + parses every file the tsconfig includes.
        // For ping mode the handle is discarded immediately; cp2
        // caches per tsconfig path.
        const { Project } = state.tsMorph;
        // Suppress noisy diagnostics during init — we only care about
        // the timing, not the project's correctness.
        const project = new Project({ tsConfigFilePath: openProject.tsconfigPath });
        // Force eager source-file resolution so the timing reflects
        // the real cost rather than lazy-load tricks.
        project.getSourceFiles();
        projectInitMs = nowMs() - initStart;
      } catch (e) {
        const err = new Error(
          `Project init failed for tsconfig ${openProject.tsconfigPath}: ${
            e instanceof Error ? e.message : String(e)
          }`,
        );
        err.code = "project_init_failed";
        throw err;
      }
    }
  }

  const totalMs = nowMs() - t0;
  return {
    tsMorphVersion: state.tsMorphVersion,
    timings: {
      tsMorphImportMs,
      projectInitMs,
      totalMs,
    },
  };
}

// Open (or return the cached) ts-morph Project for a tsconfig path.
// params: { tsconfigPath }
// result: { sourceFileCount, initMs, cached }
async function handleOpenProject(params) {
  const tsconfigPath = params.tsconfigPath;
  if (typeof tsconfigPath !== "string" || !tsconfigPath) {
    const err = new Error("openProject requires a tsconfigPath");
    err.code = "project_init_failed";
    throw err;
  }
  const { project, initMs, cached } = await getOrCreateProject(tsconfigPath);
  return {
    sourceFileCount: project.getSourceFiles().length,
    initMs: Math.round(initMs),
    cached,
  };
}

// Resolve the type of the node at a byte span.
// params: { tsconfigPath, file, startByte, endByte }
// result: TypeFacts | null
//   { text, isNullable, includesNull, includesUndefined, isAny } or null
//   when no meaningful type could be resolved.
async function handleTypeAt(params) {
  const { tsconfigPath, file, startByte, endByte } = params;
  if (typeof tsconfigPath !== "string" || typeof file !== "string") {
    const err = new Error("typeAt requires tsconfigPath and file");
    err.code = "internal";
    throw err;
  }
  const { project } = await getOrCreateProject(tsconfigPath);

  const sourceFile = resolveSourceFile(project, file);
  if (!sourceFile) {
    // File isn't part of the project and couldn't be added — the
    // caller treats a null result as "can't conclude".
    return null;
  }

  const fullText = sourceFile.getFullText();
  const startU16 = utf16PosFromByteOffset(fullText, startByte | 0);
  const endU16 = utf16PosFromByteOffset(fullText, endByte | 0);

  // Prefer an exact start+width match; fall back to the deepest node at
  // the start position. ts-morph throws if width is negative or out of
  // range, so guard.
  let node = null;
  const width = endU16 - startU16;
  try {
    if (width > 0) {
      node = sourceFile.getDescendantAtStartWithWidth(startU16, width) ?? null;
    }
  } catch {
    node = null;
  }
  if (!node) {
    try {
      node = sourceFile.getDescendantAtPos(startU16) ?? null;
    } catch {
      node = null;
    }
  }
  if (!node) return null;

  let type;
  try {
    type = node.getType();
  } catch {
    return null;
  }
  if (!type) return null;

  return typeFacts(type, node);
}

// --- helpers ----------------------------------------------------------

async function getOrCreateProject(tsconfigPath) {
  await ensureTsMorphLoaded();
  if (state.tsMorphLoadError) {
    const err = new Error(state.tsMorphLoadError);
    err.code = "ts_morph_unavailable";
    throw err;
  }
  const existing = state.projects.get(tsconfigPath);
  if (existing) {
    return { project: existing, initMs: 0, cached: true };
  }
  const initStart = nowMs();
  let project;
  try {
    const { Project } = state.tsMorph;
    project = new Project({ tsConfigFilePath: tsconfigPath });
    // Force eager source-file resolution so later typeAt lookups hit a
    // warm project rather than paying lazy-load cost mid-query.
    project.getSourceFiles();
  } catch (e) {
    const err = new Error(
      `Project init failed for tsconfig ${tsconfigPath}: ${
        e instanceof Error ? e.message : String(e)
      }`,
    );
    err.code = "project_init_failed";
    throw err;
  }
  const initMs = nowMs() - initStart;
  state.projects.set(tsconfigPath, project);
  return { project, initMs, cached: false };
}

// Find the SourceFile for `file` in `project`, tolerating path-shape
// differences (back/forward slashes, drive-letter case on Windows).
// Falls back to adding the file to the project if it exists on disk but
// wasn't picked up by the tsconfig's include globs.
function resolveSourceFile(project, file) {
  const fwd = file.replace(/\\/g, "/");
  let sf = project.getSourceFile(fwd);
  if (sf) return sf;
  const lower = fwd.toLowerCase();
  sf = project.getSourceFiles().find((f) => f.getFilePath().toLowerCase() === lower);
  if (sf) return sf;
  try {
    return project.addSourceFileAtPathIfExists(fwd) ?? null;
  } catch {
    return null;
  }
}

// Build the compact TypeFacts wire object from a ts-morph Type.
function typeFacts(type, node) {
  const unionParts = safeCall(() => type.getUnionTypes(), []) ?? [];
  const parts = unionParts.length > 0 ? unionParts : [type];
  const includesNull = parts.some((t) => safeCall(() => t.isNull(), false));
  const includesUndefined = parts.some((t) => safeCall(() => t.isUndefined(), false));
  const isAny =
    safeCall(() => type.isAny(), false) || safeCall(() => type.isUnknown(), false);
  return {
    text: safeCall(() => type.getText(node), "") ?? "",
    isNullable: safeCall(() => type.isNullable(), includesNull || includesUndefined),
    includesNull,
    includesUndefined,
    isAny,
  };
}

function safeCall(fn, fallback) {
  try {
    return fn();
  } catch {
    return fallback;
  }
}

// Translate a UTF-8 byte offset into a UTF-16 code-unit position (the
// position model the TS Compiler API uses). Walks code points,
// accumulating UTF-8 byte length until it reaches the target, summing
// UTF-16 code units (2 for astral-plane code points) along the way. For
// ASCII source the two are identical; this only matters for files with
// multi-byte characters before the queried node.
function utf16PosFromByteOffset(text, byteOffset) {
  if (byteOffset <= 0) return 0;
  let bytes = 0;
  let utf16 = 0;
  for (const ch of text) {
    const chBytes = Buffer.byteLength(ch, "utf8");
    if (bytes + chBytes > byteOffset) break;
    bytes += chBytes;
    utf16 += ch.length;
    if (bytes >= byteOffset) break;
  }
  return utf16;
}

async function ensureTsMorphLoaded() {
  if (state.tsMorph) return 0;
  if (state.tsMorphLoadError) return 0; // cached failure; caller will throw

  const importStart = nowMs();
  try {
    // Node ESM ignores NODE_PATH, so we resolve ts-morph by hand:
    // walk up from the project root (COFFERDAM_TYPE_HOST_PROJECT_ROOT)
    // looking for `node_modules/ts-morph/package.json`, then construct
    // a file:// URL to the package's main entry and import that.
    const projectRoot = process.env.COFFERDAM_TYPE_HOST_PROJECT_ROOT ?? process.cwd();
    const resolved = resolveTsMorph(projectRoot);
    if (!resolved) {
      state.tsMorphLoadError =
        `ts-morph not found in any node_modules walking up from ${projectRoot}. ` +
        `Install with: npm install --save-dev ts-morph`;
      return 0;
    }
    const mod = await import(pathToFileURL(resolved.entryPath).href);
    state.tsMorph = mod;
    state.tsMorphVersion = resolved.version;
    return nowMs() - importStart;
  } catch (e) {
    state.tsMorphLoadError =
      `ts-morph load failed: ` + (e instanceof Error ? e.message : String(e));
    return 0;
  }
}

// Walk up from `startDir`, looking for `node_modules/ts-morph/package.json`.
// Returns `{ entryPath, version }` on success — entryPath is the absolute
// path to the package's main file (or its `main`/`exports["."]` entry),
// suitable for a `pathToFileURL(...).href` import.
function resolveTsMorph(startDir) {
  let dir = resolvePath(startDir);
  for (let depth = 0; depth < 32; depth++) {
    const pkgPath = joinPath(dir, "node_modules", "ts-morph", "package.json");
    try {
      const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
      const pkgDir = dirname(pkgPath);
      const entry = pickEntry(pkg);
      const entryPath = entry
        ? joinPath(pkgDir, entry)
        : findIndexFallback(pkgDir);
      if (!entryPath) {
        continue;
      }
      try {
        statSync(entryPath);
      } catch {
        continue;
      }
      return {
        entryPath,
        version: typeof pkg.version === "string" ? pkg.version : null,
      };
    } catch {
      // try parent
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

function pickEntry(pkg) {
  // Mirror Node's ESM resolution priorities: exports["."].import, then
  // module, then main. ts-morph publishes both CJS and ESM; we prefer
  // ESM but accept CJS via Node's interop.
  const exp = pkg.exports;
  if (exp && typeof exp === "object") {
    const dot = exp["."] ?? exp;
    if (typeof dot === "string") return dot;
    if (dot && typeof dot === "object") {
      return dot.import ?? dot.default ?? dot.node ?? null;
    }
  }
  if (typeof pkg.module === "string" && pkg.module) return pkg.module;
  if (typeof pkg.main === "string" && pkg.main) return pkg.main;
  return null;
}

function findIndexFallback(pkgDir) {
  for (const name of ["index.mjs", "index.js"]) {
    const p = joinPath(pkgDir, name);
    try {
      statSync(p);
      return p;
    } catch {
      // try next
    }
  }
  return null;
}

function nowMs() {
  // High-resolution wall-clock in ms (float). Integer-cast at emission.
  return Number(process.hrtime.bigint()) / 1_000_000;
}

function writeResponse(obj) {
  // Round any timings before serialisation — wire spec uses integer ms.
  if (obj.result?.timings) {
    for (const key of Object.keys(obj.result.timings)) {
      const v = obj.result.timings[key];
      obj.result.timings[key] = v == null ? null : Math.round(v);
    }
  }
  process.stdout.write(JSON.stringify(obj) + "\n");
}
