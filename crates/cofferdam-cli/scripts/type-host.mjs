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
//
// CD-81: the ts-morph plumbing (module resolution, project caching,
// type resolution) now lives in `type-host-core.mjs`, shared with the
// plugin host (`plugin-host.mjs`), which resolves types in-process
// rather than round-tripping through this worker. The handlers below
// are thin wire-shape adapters around that shared module.

import { createInterface } from "node:readline";
import {
  createTypeHostState,
  ensureTsMorphLoaded,
  getOrCreateProject,
  typeAt as coreTypeAt,
} from "./type-host-core.mjs";

// State persisted across requests in this worker's lifetime. The
// ts-morph module is imported once and reused; Projects are cached by
// tsconfig path so the costly init (seconds on a large repo) is paid
// once per analysis run, not per query.
const state = createTypeHostState();

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
    const projectRoot = process.env.COFFERDAM_TYPE_HOST_PROJECT_ROOT ?? process.cwd();
    tsMorphImportMs = await ensureTsMorphLoaded(state, projectRoot);
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
        // caches per tsconfig path (via getOrCreateProject).
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
  // Load ts-morph against this worker's configured project root (the env
  // var `spawn_worker` in type_host.rs sets) before delegating to the
  // shared core — `getOrCreateProject` only lazy-loads against the
  // tsconfig's own directory, which may differ from the project root a
  // caller explicitly asked this worker to resolve `ts-morph` from.
  const projectRoot = process.env.COFFERDAM_TYPE_HOST_PROJECT_ROOT ?? process.cwd();
  await ensureTsMorphLoaded(state, projectRoot);
  const { project, initMs, cached } = await getOrCreateProject(state, tsconfigPath);
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
  // See handleOpenProject — ensure ts-morph is loaded against this
  // worker's configured project root before delegating. A no-op once
  // `openProject` (or an earlier `typeAt`) has already loaded it.
  const projectRoot = process.env.COFFERDAM_TYPE_HOST_PROJECT_ROOT ?? process.cwd();
  await ensureTsMorphLoaded(state, projectRoot);
  return coreTypeAt(state, tsconfigPath, file, startByte, endByte);
}

// --- helpers ----------------------------------------------------------

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
