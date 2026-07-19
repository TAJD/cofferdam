#!/usr/bin/env node
// cofferdam plugin host (cd-81a.7 / cd-7e4 / CD-33).
//
// Spawned by the cofferdam Rust CLI when `cofferdam.toml` declares a
// non-empty `plugins = [...]` array. Streams an NDJSON manifest on
// stdin (one record per line), dynamic-imports each plugin's default
// export, runs every plugin against each file as it arrives, and
// streams the merged report/error set back as NDJSON on stdout. This
// keeps both sides at O(one file) peak memory instead of O(repo) — see
// `design/sdk-ast-wire.md` for the wire spec (wireVersion 2).
//
// STREAMED MANIFEST (Rust side: HeaderRecord/ManifestFile/EndRecord in
// plugins.rs), one JSON object per stdin line:
//   {"type":"header","wireVersion":2,"cwd":"/abs/project-root",
//    "plugins":["./examples-plugins/brand-casing"],
//    "options":{"<checkId>":{"<key>":<RawOptionValue>}},
//    "tsconfigPath":"/abs/project-root/tsconfig.json"}   // null/absent when none found
//   {"type":"file","path":"/abs/file.ts","text":"...",
//    "lineViews":[{...}],"layer":null,"ast":{...}}
//   ... (one "file" record per source file, in order) ...
//   {"type":"end"}
//
// STREAMED OUTPUT (Rust side: StreamRecord in plugins.rs), one JSON
// object per stdout line, in the same order the old one-shot `reports`/
// `errors` arrays would have held them:
//   {"type":"report","checkId":"BrandCasing","message":"...",
//    "file":"/abs/.../file.ts","startByte":123,"endByte":131,
//    "severity":"high","fix":{...}}   // fix optional
//   {"type":"error","kind":"load_failed"|"run_threw"|"finalize_threw",
//    "plugin":"./examples-plugins/brand-casing",
//    "file":"/abs/.../foo.ts","message":"..."}   // file empty for load_failed
//   {"type":"done","typeHostUnavailable":null}   // typeHostUnavailable field
//     // present only when a loaded check declared requiresTypes:true;
//     // null when ts-morph resolved fine, else a human-readable reason
//     // (CD-81 — see type-host-core.mjs).
//
// A separate, still one-shot METADATA mode (`query_plugin_metadata` in
// plugins.rs) sends a single `{"mode":"metadata","cwd":...,"plugins":[...]}`
// object (no streamed files) and gets back one
// `{"checks":[...],"errors":[...]}` object — small enough that
// streaming buys nothing there, so it's untouched by CD-33.
//
// Self-contained on purpose: imports nothing from @cofferdam/check-sdk
// at the host level so it runs even when the SDK isn't hoisted at the
// project root. The plugin's own node_modules resolves the SDK for the
// plugin module's import.

import { readFileSync, statSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { resolve as resolvePath, join as joinPath } from "node:path";
import { createInterface } from "node:readline";
import {
  createTypeHostState,
  ensureTsMorphLoaded,
  typeAt as coreTypeAt,
  resolveLiteral as coreResolveLiteral,
} from "./type-host-core.mjs";

// SDK major versions this host knows how to drive (cd-b1q). Plugins
// vendor their own `@cofferdam/check-sdk` via their package.json, so
// resolution flows through the plugin's own `node_modules` tree rather
// than a cofferdam-bundled copy. This guard rejects a plugin that
// pulls in an SDK major outside this set with a loud, named error
// instead of letting it explode inside `run()` with a cryptic mismatch.
const SUPPORTED_SDK_MAJORS = new Set(["0"]);

// Map from wire-side AST node kind to the visitor method name plugins
// implement. Declared up here so it's initialised before the main
// per-file loop runs the AstView walk (TDZ matters at module scope).
const VISITOR_METHODS = {
  Program: "visitProgram",
  CallExpression: "visitCallExpression",
  ImportDeclaration: "visitImportDeclaration",
  Function: "visitFunction",
  ArrowFunctionExpression: "visitArrowFunctionExpression",
  Class: "visitClass",
  ObjectExpression: "visitObjectExpression",
  MemberExpression: "visitMemberExpression",
  IdentifierReference: "visitIdentifierReference",
  VariableDeclaration: "visitVariableDeclaration",
  JSXElement: "visitJSXElement",
  JSXAttribute: "visitJSXAttribute",
  JSXExpressionContainer: "visitJSXExpressionContainer",
  Document: "visitDocument",
  Element: "visitElement",
  Attribute: "visitAttribute",
  Text: "visitText",
  Comment: "visitComment",
  Doctype: "visitDoctype",
};

// State for the streaming run — populated by the "header" record,
// mutated per "file" record, read by "end"'s finalize pass.
let cwd = "";
let optionsCfg = {};
let filesSeen = 0;
const loadedPlugins = [];

// CD-81: type-aware plugin routing. `tsconfigPath` comes from the
// header record (Rust-side discovery, same tsconfig the built-in type
// oracle uses); `null` means no tsconfig was found / type-awareness is
// disabled. `typeHostState` is this process's own ts-morph cache — a
// separate instance from the standalone type-host worker's, since each
// is a distinct Node process (see type-host-core.mjs's module doc).
// ts-morph is loaded eagerly, once, right after `loadPlugins` in the
// header handler (only when a loaded check declares `requiresTypes`),
// so `ctx.types` is ready before the first "file" record arrives.
const typeHostState = createTypeHostState();
let tsconfigPath = null;
let typeHostUnavailableReason = null;
let typeHostUnavailableReported = false;
let anyCheckRequiresTypes = false;
const corpusStores = new Map();
// CD-71: per-check count of files that matched `check.files`'s include
// patterns, so a plugin whose pathPatterns typo'd into matching nothing
// (e.g. the CD-70 bare-`**` bug) gets a loud warning instead of silently
// never firing.
const scopeMatchCounts = new Map();
// Dump the wire payload as JSON to a file when COFFERDAM_PLUGIN_HOST_DUMP_WIRE
// is set — used by scripts/check-ast-spans.mjs to verify byte-range
// round-trip without instrumenting the host script's main path. cd-svf
// span round-trip CI guardrail. Accumulated across "file" records and
// flushed at "end" (this is a debug-only path; it re-introduces O(repo)
// memory deliberately, gated behind the env var).
const dumpEntries = process.env.COFFERDAM_PLUGIN_HOST_DUMP_WIRE ? [] : null;

function writeRecord(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

// Write a record and resolve only once the stream has handed its bytes off
// to the OS (the write callback fired). Used for the terminal records — the
// streaming `done` handshake and the one-shot metadata response — because
// on POSIX a pipe write from `process.stdout` is asynchronous (Node docs:
// "Pipes (and sockets): asynchronous on POSIX"). Relying on natural
// event-loop drain to flush that final write before exit is only
// incidentally safe: the record-processing promise chain is floating (see
// the `rl.on("line")` block), so nothing structurally anchors the loop to
// the moment `done` is actually out. Under heavy load an interleaving
// exists where the loop reaches its exit check with the chain suspended and
// no live libuv handle keeping it alive, so the process exits 0 with `done`
// still buffered — the Rust reader then sees EOF without the completion
// marker and raises a spurious Warning.PluginHostFailed (the CD-41
// "completion marker" race). Awaiting the write callback makes the pending
// write itself the anchor: the chain cannot complete — and the process
// cannot go idle — until the bytes are confirmed flushed. Stdout preserves
// write order, so every report queued before it has flushed by then too.
function writeRecordFlushed(obj) {
  return new Promise((resolve, reject) => {
    process.stdout.write(JSON.stringify(obj) + "\n", (err) => {
      if (err) reject(err);
      else resolve();
    });
  });
}

// cd-9hp.6: per-plugin corpus stores survive every per-file `run` and
// are reused by the optional `finalize` hook. Storage is plugin-private
// — two plugins picking the same slot key cannot see each other's data.
async function loadPlugins(pluginPaths) {
  const errors = [];
  for (const pluginPath of pluginPaths) {
    try {
      const pluginDir = resolvePath(cwd, pluginPath);
      const sdkCheck = checkPluginSdkMajor(pluginDir);
      if (sdkCheck.kind === "incompatible") {
        throw new Error(sdkCheck.message);
      }
      const resolved = resolveEntryPoint(pluginDir);
      const url = pathToFileURL(resolved).href;
      const mod = await import(url);
      const check = mod.default;
      if (!check || typeof check.run !== "function" || typeof check.id !== "string") {
        throw new Error(
          `module's default export is not a Check object (missing id/run). Got keys: ${
            check ? Object.keys(check).join(", ") : "no default export"
          }`,
        );
      }
      loadedPlugins.push({ pluginPath, check });
      corpusStores.set(check.id, buildCorpusStore());
      scopeMatchCounts.set(check.id, 0);
    } catch (err) {
      errors.push({
        kind: "load_failed",
        plugin: pluginPath,
        file: "",
        message: describeLoadError(err),
      });
    }
  }
  return errors;
}

// CD-72: Node's dynamic `import()` parses a `.js` file as CommonJS unless
// the nearest package.json declares `"type": "module"` — a plugin authored
// with `import`/`export` syntax but missing that field fails with one of
// these syntax errors, which give no hint about the actual fix. Detect the
// known error shapes and append one.
function describeLoadError(err) {
  const message = err instanceof Error ? err.message : String(err);
  const looksLikeMissingTypeModule =
    /Cannot use import statement outside a module/.test(message) ||
    /Unexpected token ['"]export['"]/.test(message) ||
    /exports is not defined/.test(message) ||
    /module is not defined/.test(message);
  if (!looksLikeMissingTypeModule) return message;
  return (
    `${message} ` +
    `(hint: this looks like ESM syntax loaded as CommonJS — add ` +
    `"type": "module" to the plugin package's package.json)`
  );
}

async function processFile(rec) {
  if (dumpEntries) dumpEntries.push({ path: rec.path, text: rec.text, ast: rec.ast ?? null });
  if (process.env.COFFERDAM_PLUGIN_HOST_DEBUG) {
    process.stderr.write(
      `[host] file=${rec.path} lines=${rec.lineViews.length} ` +
        `astNodes=${rec.ast?.nodes?.length ?? 0} plugins=${loadedPlugins.length}\n`,
    );
  }
  filesSeen++;
  const sourceFile = buildSourceFile(rec);
  for (const { pluginPath, check } of loadedPlugins) {
    if (!fileMatchesScope(rec.path, check.files, rec.layer ?? null)) continue;
    scopeMatchCounts.set(check.id, (scopeMatchCounts.get(check.id) ?? 0) + 1);

    const opts = resolveOptions(check, optionsCfg);
    const corpusStore = corpusStores.get(check.id);
    const ctx = {
      report(args) {
        if (!args || typeof args !== "object") return;
        const span = args.span;
        if (!span) return;
        const out = {
          type: "report",
          checkId: check.id,
          category: check.category ?? "warning",
          message: String(args.message ?? ""),
          file: rec.path,
          startByte: Number(span.start_byte ?? 0) | 0,
          endByte: Number(span.end_byte ?? 0) | 0,
          severity: args.severity ?? check.defaultSeverity ?? "medium",
        };
        if (args.fix) out.fix = args.fix;
        if (args.related) out.related = args.related;
        writeRecord(out);
      },
      corpus: corpusStore.view(),
    };
    // CD-81: `ctx.types` is present only when this check declared
    // `requiresTypes: true` AND a tsconfig was found AND ts-morph
    // loaded successfully. Absent (not just null) otherwise — plugins
    // test `if (ctx.types)` rather than a null-check per call.
    if (check.requiresTypes === true && tsconfigPath && !typeHostState.tsMorphLoadError) {
      ctx.types = {
        async typeAt(startByte, endByte) {
          return coreTypeAt(typeHostState, tsconfigPath, rec.path, startByte, endByte);
        },
        // CD-82: resolves an identifier/import reference to a literal
        // value (cross-file, via ts-morph symbol resolution). Same
        // in-process pattern as `typeAt` above — no new IPC hop.
        async resolveLiteral(startByte, endByte) {
          return coreResolveLiteral(typeHostState, tsconfigPath, rec.path, startByte, endByte);
        },
      };
    }

    try {
      await check.run(sourceFile, ctx, opts);
    } catch (err) {
      writeRecord({
        type: "error",
        kind: "run_threw",
        plugin: pluginPath,
        file: rec.path,
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }
}

// cd-9hp.6: finalize pass. After every per-file run has completed,
// invoke each plugin's optional `finalize(ctx, opts)` once. Findings
// emitted here attach to whatever file the plugin supplies via
// `args.file`; the host does not infer a primary file.
async function finalizeAndEmit() {
  for (const { pluginPath, check } of loadedPlugins) {
    if (typeof check.finalize !== "function") continue;
    const opts = resolveOptions(check, optionsCfg);
    const corpusStore = corpusStores.get(check.id);
    const ctx = {
      report(args) {
        if (!args || typeof args !== "object") return;
        const span = args.span;
        if (!span) return;
        const out = {
          type: "report",
          checkId: check.id,
          category: check.category ?? "warning",
          message: String(args.message ?? ""),
          file: typeof args.file === "string" ? args.file : "",
          startByte: Number(span.start_byte ?? 0) | 0,
          endByte: Number(span.end_byte ?? 0) | 0,
          severity: args.severity ?? check.defaultSeverity ?? "medium",
        };
        if (args.fix) out.fix = args.fix;
        if (args.related) out.related = args.related;
        writeRecord(out);
      },
      corpus: corpusStore.view(),
    };

    try {
      // CD-91: `finalize` may be `async` (TS's void-return compatibility
      // rule lets that typecheck against `Check.finalize`'s sync-or-void
      // signature) — awaiting via `Promise.resolve(...)` handles both a
      // synchronous return and a real Promise uniformly, the same
      // treatment `run()` already gets. Without this, a rejected async
      // finalize never surfaced as `finalize_threw`, and late `ctx.report`
      // calls inside it could race the host's closing `done` record.
      await Promise.resolve(check.finalize(ctx, opts));
    } catch (err) {
      writeRecord({
        type: "error",
        kind: "finalize_threw",
        plugin: pluginPath,
        file: "",
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }

  // CD-71: a check whose `files.pathPattern(s)` matched zero of the
  // discovered files never reports anything, indistinguishable from "no
  // violations found" — surface it as a warning instead of silence.
  if (filesSeen > 0) {
    for (const { pluginPath, check } of loadedPlugins) {
      if (!hasIncludePatterns(check.files) || (scopeMatchCounts.get(check.id) ?? 0) > 0) continue;
      writeRecord({
        type: "error",
        kind: "zero_scope_match",
        plugin: pluginPath,
        file: "",
        message: `plugin check '${check.id}' matched 0 of ${filesSeen} discovered file(s) — check files.pathPattern(s) for a typo or an unreachable glob`,
      });
    }
  }

  if (dumpEntries) {
    const dumpPath = process.env.COFFERDAM_PLUGIN_HOST_DUMP_WIRE;
    const { writeFileSync } = await import("node:fs");
    writeFileSync(dumpPath, JSON.stringify(dumpEntries, null, 2));
  }

  // CD-81: only report type-host status when at least one loaded check
  // actually declared `requiresTypes: true` — no point surfacing
  // ts-morph availability when nothing needed it.
  const done = { type: "done" };
  if (anyCheckRequiresTypes) done.typeHostUnavailable = typeHostUnavailableReason;
  await writeRecordFlushed(done);
}

// Idempotent finalize gate. `finalizeAndEmit` must run exactly once even
// though it can be reached from two places: the terminal `end` record and
// the stdin `close` safety net (below). The flag is set synchronously on
// entry, before any await, so a second concurrent-looking call on the same
// promise chain is a guaranteed no-op.
let finalized = false;
async function finalizeOnce() {
  if (finalized) return;
  finalized = true;
  await finalizeAndEmit();
}

// Record the type-host-unavailable reason once (module state) and emit
// exactly one stderr diagnostic across the whole run, no matter how many
// files/checks would have wanted `ctx.types`.
function reportTypeHostUnavailableOnce(reason) {
  typeHostUnavailableReason = reason;
  if (typeHostUnavailableReported) return;
  typeHostUnavailableReported = true;
  process.stderr.write(
    `[cofferdam] type-aware plugin check(s) declared requiresTypes:true but the type host is ` +
      `unavailable (${reason}) — those checks will run with ctx.types undefined.\n`,
  );
}

// Metadata mode: return check metadata without processing any files.
// Request shape: { "mode": "metadata", "cwd": "...", "plugins": [...] }
// Response shape: {
//   "checks": [{
//     path, id, category, basePriority, defaultSeverity, explanation, body,
//     requiresTypes, outputMode, files,
//     options: [{ name, kind, default, doc }]
//   }],
//   "errors": [...]
// }
async function runMetadataMode(rec) {
  cwd = rec.cwd;
  const errors = await loadPlugins(rec.plugins ?? []);
  const checks = loadedPlugins.map(({ pluginPath, check }) => ({
    path: pluginPath,
    id: check.id,
    category: check.category ?? "warning",
    basePriority: check.basePriority ?? 0,
    defaultSeverity: check.defaultSeverity ?? "medium",
    explanation: check.explanation ?? "",
    body: check.body ?? null,
    requiresTypes: check.requiresTypes ?? false,
    outputMode: check.outputMode ?? false,
    files: check.files ?? null,
    options: check.options
      ? Object.entries(check.options).map(([name, spec]) => ({
          name,
          kind: spec.kind ?? "string",
          default: spec.default ?? null,
          doc: spec.doc ?? "",
        }))
      : [],
  }));
  // Await the flush (see `writeRecordFlushed`) rather than fire-and-forget:
  // the metadata response is this mode's entire output and its last write,
  // so the same "process goes idle before the async pipe write drains"
  // window that bites the streaming `done` record applies here too. The
  // Rust metadata reader parses the whole stdout as one JSON object, so a
  // truncated response is an unconditional parse failure.
  await writeRecordFlushed({ checks, errors });
  // cd-41: don't force-exit right after the write. Writes to a piped
  // stdout are asynchronous on POSIX — process.exit() can outrun the OS
  // flush and truncate the very output we just wrote, especially under
  // the CPU contention of several concurrent hosts. Setting exitCode and
  // letting the event loop drain naturally means the process only exits
  // once the write (and everything else pending) has actually completed.
  process.exitCode = 0;
}

async function handleRecord(rec) {
  if (rec.mode === "metadata") {
    await runMetadataMode(rec);
    return;
  }
  switch (rec.type) {
    case "header": {
      sawHeader = true;
      cwd = rec.cwd;
      optionsCfg = rec.options ?? {};
      tsconfigPath = typeof rec.tsconfigPath === "string" ? rec.tsconfigPath : null;
      const errors = await loadPlugins(rec.plugins ?? []);
      for (const e of errors) writeRecord({ type: "error", ...e });

      anyCheckRequiresTypes = loadedPlugins.some(({ check }) => check.requiresTypes === true);
      // Load ts-morph once, up front, so every `ctx.types.typeAt` call
      // during `processFile` can await an already-resolved import
      // instead of re-triggering the dynamic import per call.
      if (anyCheckRequiresTypes && tsconfigPath) {
        await ensureTsMorphLoaded(typeHostState, cwd);
        if (typeHostState.tsMorphLoadError) {
          reportTypeHostUnavailableOnce(typeHostState.tsMorphLoadError);
        }
      } else if (anyCheckRequiresTypes && !tsconfigPath) {
        reportTypeHostUnavailableOnce("no tsconfig.json found for type-aware plugin checks");
      }
      break;
    }
    case "file":
      await processFile(rec);
      break;
    case "end":
      await finalizeOnce();
      // cd-41: see the comment in runMetadataMode — avoid forcing exit
      // right after finalizeAndEmit's final `done` write so it can't be
      // truncated by an async pipe flush racing process.exit().
      process.exitCode = 0;
      break;
    default:
      process.stderr.write(`plugin-host: unknown stream record type: ${rec.type}\n`);
  }
}

// Records are processed strictly in arrival order — each line's handler
// (which may be async, e.g. the header's dynamic `import()`s) must
// finish before the next line is handled, or a "file" record could run
// against a not-yet-loaded plugin list.
let chain = Promise.resolve();
let sawHeader = false;
const rl = createInterface({ input: process.stdin, terminal: false });
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  chain = chain.then(async () => {
    let rec;
    try {
      rec = JSON.parse(trimmed);
    } catch (e) {
      process.stderr.write(`plugin-host: bad record JSON: ${e.message}\n`);
      return;
    }
    await handleRecord(rec);
  });
  chain = chain.catch((e) => {
    process.stderr.write(`plugin-host: internal error: ${e instanceof Error ? e.stack : e}\n`);
    process.exitCode = 1;
  });
});

// Stdin fully consumed. Chain a final `finalizeOnce` after every queued
// record handler so a streaming run always emits (and flushes) its closing
// `done` handshake before the process is allowed to go idle — even if the
// terminal `end` record never arrived (a manifest cut short by a write
// error on the Rust side). The `finalized` guard makes this a no-op when
// `end` already triggered finalize, and `sawHeader` keeps it from firing in
// metadata mode (which has no `header`/`end` protocol and whose extra
// `done` line would corrupt the single-object response the Rust side
// parses). Because `finalizeAndEmit` awaits its `done` write via
// `writeRecordFlushed`, the pending write anchors the event loop through
// completion — this is the structural guarantee the previously-floating
// chain lacked.
rl.on("close", () => {
  if (!sawHeader) return;
  chain = chain
    .then(() => finalizeOnce())
    .catch((e) => {
      process.stderr.write(
        `plugin-host: finalize on close failed: ${e instanceof Error ? e.stack : e}\n`,
      );
      process.exitCode = 1;
    });
});

// ---- helpers --------------------------------------------------------

/**
 * Walk up from `pluginDir` looking for `node_modules/@cofferdam/check-sdk/package.json`,
 * then verify its `version`'s major component is in `SUPPORTED_SDK_MAJORS`.
 *
 * Returns one of:
 *   { kind: "ok", version }      — found and compatible (or absent — see below)
 *   { kind: "incompatible", message }  — found but SDK major outside the set
 *
 * The "absent" case is treated as ok because Node's own `import()` will
 * raise a clear `Cannot find package '@cofferdam/check-sdk'` if the SDK
 * isn't actually resolvable when the plugin is loaded — no need to
 * duplicate that error here. We only want to short-circuit when we *can*
 * see a wrong-major install before letting the dynamic import succeed
 * against type defs that don't match the runtime contract this host
 * speaks (cd-b1q acceptance: loud, named error, not a silent crash
 * inside run()).
 */
function checkPluginSdkMajor(pluginDir) {
  let dir = pluginDir;
  for (let depth = 0; depth < 16; depth++) {
    const pkgPath = joinPath(dir, "node_modules", "@cofferdam", "check-sdk", "package.json");
    try {
      const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
      const version = typeof pkg.version === "string" ? pkg.version : "";
      const major = version.split(".")[0] ?? "";
      if (!SUPPORTED_SDK_MAJORS.has(major)) {
        return {
          kind: "incompatible",
          message:
            `plugin's @cofferdam/check-sdk@${version} is incompatible with this cofferdam ` +
            `(supported SDK majors: ${[...SUPPORTED_SDK_MAJORS].join(", ")}). ` +
            `Update the plugin to a compatible SDK version, or upgrade cofferdam.`,
        };
      }
      return { kind: "ok", version };
    } catch {
      // try parent
    }
    const parent = joinPath(dir, "..");
    if (parent === dir) break;
    dir = parent;
  }
  return { kind: "ok", version: null };
}

function resolveEntryPoint(absPath) {
  // Node's ESM `import()` rejects directory imports — resolve them via
  // `package.json#main` (then `module`, then `exports["."]`) the same
  // way CommonJS would. Falls back to `index.js`/`index.mjs` if no
  // package.json is present. Throws on a missing entry so the caller
  // can surface a `load_failed` error.
  let stat;
  try {
    stat = statSync(absPath);
  } catch {
    return absPath;
  }
  if (!stat.isDirectory()) return absPath;

  const pkgPath = joinPath(absPath, "package.json");
  try {
    const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
    const candidate = pickEntry(pkg);
    if (candidate) return joinPath(absPath, candidate);
  } catch {
    // No package.json; fall through to the index.* fallback.
  }
  for (const fallback of ["index.mjs", "index.js"]) {
    try {
      const candidate = joinPath(absPath, fallback);
      statSync(candidate);
      return candidate;
    } catch {
      /* try next */
    }
  }
  throw new Error(
    `no entry point found in '${absPath}' (no package.json#main, no index.{js,mjs})`,
  );
}

function pickEntry(pkg) {
  if (typeof pkg.main === "string" && pkg.main) return pkg.main;
  if (typeof pkg.module === "string" && pkg.module) return pkg.module;
  const exp = pkg.exports;
  if (exp && typeof exp === "object") {
    const dot = exp["."] ?? exp;
    if (typeof dot === "string") return dot;
    if (dot && typeof dot === "object") {
      return dot.default ?? dot.import ?? null;
    }
  }
  return null;
}

function buildSourceFile(file) {
  const lineViews = file.lineViews.map(buildLineView);
  return {
    path: file.path,
    text: file.text,
    layer: file.layer ?? null,
    lines() {
      let i = 0;
      const it = {
        next() {
          if (i < lineViews.length) return { value: lineViews[i++], done: false };
          return { value: undefined, done: true };
        },
        [Symbol.iterator]() {
          return it;
        },
      };
      return it;
    },
    ast: file.ast ? buildAstView(file.ast) : null,
  };
}

// AstView reconstruction (cd-svf). The wire ships a flat array of nodes
// with firstChild/nextSibling indices and per-kind typed extras. This
// builds the typed object graph plugins program against:
//
//   findAll<K>(kind): returns nodes of that kind in document order
//   walk(visitor):     depth-first traversal, honours Walk.Skip
//   root:              nodes[rootIdx], the Program node
//
// Children referenced by per-kind extras (calleeIdx, paramIdxs, etc.)
// are rehydrated as typed object references via lazy getters so deeply
// nested ASTs don't pay for objects plugins never touch.
function buildAstView(wire) {
  const { rootIdx, nodes } = wire;
  const built = new Array(nodes.length);

  function get(idx) {
    if (idx < 0 || idx >= nodes.length) return null;
    if (built[idx]) return built[idx];
    const w = nodes[idx];
    const out = { kind: w.kind, span: w.span };
    switch (w.kind) {
      case "Program": {
        Object.defineProperty(out, "body", {
          enumerable: true,
          get: () => collectChildren(idx),
        });
        break;
      }
      case "CallExpression": {
        Object.defineProperty(out, "callee", {
          enumerable: true,
          get: () => get(w.calleeIdx),
        });
        Object.defineProperty(out, "arguments", {
          enumerable: true,
          get: () => (w.argumentIdxs ?? []).map(get).filter((n) => n !== null),
        });
        break;
      }
      case "ImportDeclaration": {
        out.source = w.source;
        out.specifiers = w.specifiers ?? [];
        break;
      }
      case "Function": {
        out.name = w.name ?? undefined;
        out.async = !!w.async;
        out.generator = !!w.generator;
        Object.defineProperty(out, "params", {
          enumerable: true,
          get: () => (w.paramIdxs ?? []).map(get).filter((n) => n !== null),
        });
        break;
      }
      case "ArrowFunctionExpression": {
        out.async = !!w.async;
        out.expression = !!w.expression;
        Object.defineProperty(out, "params", {
          enumerable: true,
          get: () => (w.paramIdxs ?? []).map(get).filter((n) => n !== null),
        });
        break;
      }
      case "Class": {
        out.name = w.name ?? undefined;
        break;
      }
      case "ObjectExpression": {
        Object.defineProperty(out, "properties", {
          enumerable: true,
          get: () => (w.propertyIdxs ?? []).map(get).filter((n) => n !== null),
        });
        break;
      }
      case "MemberExpression": {
        out.property = w.property ?? undefined;
        out.computed = !!w.computed;
        Object.defineProperty(out, "object", {
          enumerable: true,
          get: () => get(w.objectIdx),
        });
        break;
      }
      case "IdentifierReference": {
        out.name = w.name;
        break;
      }
      case "VariableDeclaration": {
        out.declarationKind = w.declarationKind;
        out.declarations = (w.declarations ?? []).map((d) => ({
          name: d.name ?? undefined,
          init: get(d.initIdx ?? -1) ?? undefined,
        }));
        break;
      }
      case "JSXElement": {
        out.tagName = w.tagName;
        out.selfClosing = !!w.selfClosing;
        Object.defineProperty(out, "attributes", {
          enumerable: true,
          get: () => (w.attributeIdxs ?? []).map(get).filter((n) => n !== null),
        });
        break;
      }
      case "JSXAttribute": {
        out.name = w.name ?? undefined;
        out.value = w.value ?? undefined;
        out.isExpression = !!w.isExpression;
        out.isSpread = !!w.isSpread;
        break;
      }
      case "JSXExpressionContainer": {
        break;
      }
      case "Document": {
        Object.defineProperty(out, "children", {
          enumerable: true,
          get: () => collectChildren(idx).filter((n) => n.kind !== "Attribute"),
        });
        break;
      }
      case "Element": {
        out.tagName = w.tagName;
        out.selfClosing = !!w.selfClosing;
        Object.defineProperty(out, "attributes", {
          enumerable: true,
          get: () => (w.attributeIdxs ?? []).map(get).filter((n) => n !== null),
        });
        Object.defineProperty(out, "children", {
          enumerable: true,
          get: () => collectChildren(idx).filter((n) => n.kind !== "Attribute"),
        });
        break;
      }
      case "Attribute": {
        out.name = w.name ?? undefined;
        out.value = w.value ?? undefined;
        break;
      }
      case "Text": {
        out.text = w.text;
        break;
      }
      case "Comment": {
        out.text = w.text;
        break;
      }
      case "Doctype": {
        out.text = w.text;
        break;
      }
    }
    built[idx] = out;
    return out;
  }

  function collectChildren(idx) {
    const out = [];
    let cursor = nodes[idx]?.firstChild ?? -1;
    while (cursor >= 0) {
      const node = get(cursor);
      if (node) out.push(node);
      cursor = nodes[cursor].nextSibling;
    }
    return out;
  }

  // Pre-build a kind index for findAll. O(N) one-pass; subsequent
  // findAll calls are O(1) lookups + O(M) hydration of M matching nodes.
  const indexByKind = new Map();
  for (let i = 0; i < nodes.length; i++) {
    const k = nodes[i].kind;
    if (!indexByKind.has(k)) indexByKind.set(k, []);
    indexByKind.get(k).push(i);
  }

  return {
    get root() {
      return get(rootIdx);
    },
    findAll(kind) {
      const idxs = indexByKind.get(kind) ?? [];
      return idxs.map(get);
    },
    walk(visitor) {
      const visit = (idx) => {
        if (idx < 0) return;
        const w = nodes[idx];
        const fn = visitorMethod(visitor, w.kind);
        const node = get(idx);
        const decision = fn ? fn.call(visitor, node) : "continue";
        if (decision !== "skip" && w.firstChild >= 0) visit(w.firstChild);
        visit(w.nextSibling);
      };
      visit(rootIdx);
    },
  };
}

function visitorMethod(visitor, kind) {
  const name = VISITOR_METHODS[kind];
  if (!name) return null;
  const fn = visitor[name];
  return typeof fn === "function" ? fn : null;
}

function buildLineView(native) {
  return {
    lineNo: native.lineNo,
    text: native.text,
    isComment: native.isComment,
    isDocComment: native.isDocComment,
    isStringLiteral: native.isStringLiteral,
    stringLiteralRanges: native.stringLiteralRanges ?? [],
    isJsxText: native.isJsxText,
    isPragma: native.isPragma,
    isTag: native.isTag,
    isText: native.isText,
    spanFor(charStart, charEnd) {
      return {
        line: native.lineNo,
        column: charStart + 1,
        start_byte: native.lineStart + charStart,
        end_byte: native.lineStart + charEnd,
      };
    },
  };
}

// cd-9hp.6: plugin corpus store. One per plugin per analysis run.
// Plugin-private: every `view()` reads/writes the same underlying Map
// for this store, but the host hands a distinct store to each plugin,
// so two plugins picking the same slot key never collide.
function buildCorpusStore() {
  const slots = new Map();
  return {
    view() {
      return {
        read(key) {
          return slots.get(String(key));
        },
        write(key, value) {
          slots.set(String(key), value);
        },
        append(key, item) {
          const k = String(key);
          const existing = slots.get(k);
          if (Array.isArray(existing)) {
            existing.push(item);
          } else {
            slots.set(k, [item]);
          }
        },
      };
    },
  };
}

function resolveOptions(check, perCheckOverrides) {
  const out = {};
  const schema = check.options ?? {};
  const overrides = perCheckOverrides[check.id] ?? {};
  for (const [key, spec] of Object.entries(schema)) {
    out[key] = key in overrides ? overrides[key] : spec.default;
  }
  return out;
}

function hasIncludePatterns(scope) {
  if (!scope) return false;
  if (typeof scope.pathPattern === "string" && scope.pathPattern) return true;
  return Array.isArray(scope.pathPatterns) && scope.pathPatterns.some((p) => typeof p === "string" && p);
}

function fileMatchesScope(absFilePath, scope, layer = null) {
  if (!scope) return true;

  // Normalise to forward slashes for consistent matching on all platforms.
  const fwd = absFilePath.replace(/\\/g, "/");

  const exts = scope.extensions;
  if (Array.isArray(exts) && exts.length > 0) {
    const lower = fwd.toLowerCase();
    if (!exts.some((e) => lower.endsWith("." + String(e).toLowerCase()))) return false;
  }

  // Layer pre-filter (cd-4if): if `scope.layers` is non-empty, the file's
  // resolved layer must be in the set. Files outside every declared layer
  // (layer === null) never match a non-empty `layers` filter.
  const layers = scope.layers;
  if (Array.isArray(layers) && layers.length > 0) {
    if (layer === null || !layers.includes(layer)) return false;
  }

  // Build the combined include set from pathPattern (singular, deprecated)
  // and pathPatterns (plural). A file is in-scope when the include set is
  // empty OR the file matches at least one include pattern.
  const includes = [];
  if (typeof scope.pathPattern === "string" && scope.pathPattern) {
    includes.push(scope.pathPattern);
  }
  if (Array.isArray(scope.pathPatterns)) {
    for (const p of scope.pathPatterns) {
      if (typeof p === "string" && p) includes.push(p);
    }
  }

  if (includes.length > 0 && !includes.some((pat) => globMatch(pat, fwd))) return false;

  // excludePatterns: if any matches, skip the file regardless of includes.
  const excludes = scope.excludePatterns;
  if (Array.isArray(excludes) && excludes.length > 0) {
    if (excludes.some((pat) => typeof pat === "string" && globMatch(pat, fwd))) return false;
  }

  return true;
}

// Gitignore-style glob matching. Supports:
//   **   — matches any number of path segments (including zero)
//   *    — matches any chars within a single segment (no `/`)
//   ?    — matches any single char (no `/`)
//   [..] — character class
//   {a,b}— brace expansion (non-nested, top-level only)
//
// `path` must already be normalised to forward slashes.
//
// Anchoring semantics (gitignore-compatible): patterns that do not start
// with `/` or `**/` are automatically tried with a `**/` prefix so they
// match anywhere in the path tree, not just at the root. For example,
// `lib/foo.ts` matches `/abs/project/lib/foo.ts` because we also test
// `**\/lib/foo.ts`.
//
// (Note: `**/` must not appear inside a JSDoc block comment — it terminates
// the block — so this function uses line comments instead.)
function globMatch(pattern, path) {
  // Expand top-level brace alternatives `{a,b,c}` first. Only the first
  // pair of outermost braces is expanded — nested braces are rare in
  // practice and left to the literal matcher (they won't accidentally
  // crash it, they just won't match).
  const braceMatch = pattern.match(/^(.*?)\{([^{}]+)\}(.*)$/);
  if (braceMatch) {
    const [, pre, inner, post] = braceMatch;
    return inner.split(",").some((alt) => globMatch(pre + alt + post, path));
  }
  // For patterns that are relative (no leading `/` or `**/`), also test
  // with `**/` prepended so the pattern can match any suffix of the path.
  if (!pattern.startsWith("/") && !pattern.startsWith("**/")) {
    if (globMatchSingle("**/" + pattern, path)) return true;
  }
  return globMatchSingle(pattern, path);
}

function globMatchSingle(pattern, path) {
  // Convert the glob pattern to a RegExp.
  let re = "^";
  let i = 0;
  while (i < pattern.length) {
    const c = pattern[i];
    if (c === "*") {
      if (pattern[i + 1] === "*") {
        // `**` — match any path segment sequence, including empty.
        // Adjacent slashes around `**` are collapsed by trimming the
        // surrounding `/` so `a/**/b` matches `a/b` as well as `a/x/b`.
        i += 2;
        if (pattern[i] === "/") i++; // consume trailing slash
        if (i >= pattern.length) {
          // Trailing `**` (e.g. `dir/**`) — gitignore semantics match
          // everything under `dir`, including a file directly inside it
          // (CD-70). `(?:.+/)?` below requires either nothing or full
          // segments ending in `/`, which can never match a bare trailing
          // filename — so the trailing case gets an unrestricted `.*`.
          re += ".*";
        } else {
          re += "(?:.+/)?"; // zero or more segments with trailing slash
        }
        continue;
      }
      // Single `*` — match anything except `/`.
      re += "[^/]*";
    } else if (c === "?") {
      re += "[^/]";
    } else if (c === "[") {
      // Character class — copy through until `]`.
      const end = pattern.indexOf("]", i + 1);
      if (end === -1) {
        // Unmatched `[` — treat as literal.
        re += "\\[";
      } else {
        re += pattern.slice(i, end + 1);
        i = end;
      }
    } else {
      // Escape regex metacharacters.
      re += c.replace(/[.+^${}()|\\]/g, "\\$&");
    }
    i++;
  }
  re += "$";

  try {
    return new RegExp(re).test(path);
  } catch {
    // Malformed pattern — fail safe (no match).
    return false;
  }
}
