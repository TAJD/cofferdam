#!/usr/bin/env node
// cofferdam plugin host (cd-81a.7 / cd-7e4).
//
// Spawned by the cofferdam Rust CLI when `cofferdam.toml` declares a
// non-empty `plugins = [...]` array. Reads a JSON manifest from stdin,
// dynamic-imports each plugin's default export, runs every plugin
// against every file, and emits the merged report set as JSON on
// stdout.
//
// MANIFEST shape (Rust side: PluginManifest in plugins.rs):
//   {
//     "cwd": "/abs/path/to/project-root",
//     "plugins": ["./examples-plugins/brand-casing"],
//     "files": [
//       {
//         "path": "/abs/path/to/file.ts",
//         "text": "...",
//         "lineViews": [
//           { "lineNo": 1, "text": "...", "isComment": false,
//             "isDocComment": false, "isStringLiteral": true,
//             "isJsxText": false, "isPragma": false, "lineStart": 0 },
//           ...
//         ]
//       }
//     ],
//     "options": { "<checkId>": { "<key>": <RawOptionValue> } }
//   }
//
// OUTPUT shape (parsed by plugins.rs::parse_host_response):
//   {
//     "reports": [
//       { "checkId": "BrandCasing", "message": "...",
//         "file": "/abs/.../file.ts",
//         "startByte": 123, "endByte": 131,
//         "severity": "high",
//         "fix": { "span": {...}, "replacement": "..." }   // optional
//       }
//     ],
//     "errors": [
//       { "kind": "load_failed" | "run_threw",
//         "plugin": "./examples-plugins/brand-casing",
//         "file": "/abs/.../foo.ts",   // empty for load_failed
//         "message": "..." }
//     ]
//   }
//
// Self-contained on purpose: imports nothing from @cofferdam/check-sdk
// at the host level so it runs even when the SDK isn't hoisted at the
// project root. The plugin's own node_modules resolves the SDK for the
// plugin module's import.

import { readFileSync, statSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { resolve as resolvePath, join as joinPath } from "node:path";

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
};

const manifest = readManifest();
const reports = [];
const errors = [];

const loadedPlugins = [];
for (const pluginPath of manifest.plugins) {
  try {
    const resolved = resolveEntryPoint(resolvePath(manifest.cwd, pluginPath));
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
  } catch (err) {
    errors.push({
      kind: "load_failed",
      plugin: pluginPath,
      file: "",
      message: err instanceof Error ? err.message : String(err),
    });
  }
}

if (process.env.COFFERDAM_PLUGIN_HOST_DEBUG) {
  for (const file of manifest.files) {
    process.stderr.write(
      `[host] file=${file.path} lines=${file.lineViews.length} ` +
        `astNodes=${file.ast?.nodes?.length ?? 0} plugins=${loadedPlugins.length}\n`,
    );
  }
}

// Dump the wire payload as JSON to a file when COFFERDAM_PLUGIN_HOST_DUMP_WIRE
// is set — used by scripts/check-ast-spans.mjs to verify byte-range
// round-trip without instrumenting the host script's main path. cd-svf
// span round-trip CI guardrail.
if (process.env.COFFERDAM_PLUGIN_HOST_DUMP_WIRE) {
  const dumpPath = process.env.COFFERDAM_PLUGIN_HOST_DUMP_WIRE;
  const { writeFileSync } = await import("node:fs");
  writeFileSync(
    dumpPath,
    JSON.stringify(
      manifest.files.map((f) => ({ path: f.path, text: f.text, ast: f.ast })),
      null,
      2,
    ),
  );
}

for (const file of manifest.files) {
  for (const { pluginPath, check } of loadedPlugins) {
    if (!fileMatchesScope(file.path, check.files)) continue;

    const opts = resolveOptions(check, manifest.options ?? {});
    const sourceFile = buildSourceFile(file);
    const ctx = {
      report(args) {
        if (!args || typeof args !== "object") return;
        const span = args.span;
        if (!span) return;
        const out = {
          checkId: check.id,
          category: check.category ?? "warning",
          message: String(args.message ?? ""),
          file: file.path,
          startByte: Number(span.start_byte ?? 0) | 0,
          endByte: Number(span.end_byte ?? 0) | 0,
          severity: args.severity ?? check.defaultSeverity ?? "medium",
        };
        if (args.fix) out.fix = args.fix;
        if (args.related) out.related = args.related;
        reports.push(out);
      },
    };

    try {
      check.run(sourceFile, ctx, opts);
    } catch (err) {
      errors.push({
        kind: "run_threw",
        plugin: pluginPath,
        file: file.path,
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }
}

process.stdout.write(JSON.stringify({ reports, errors }) + "\n");

// ---- helpers --------------------------------------------------------

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

function readManifest() {
  const raw = readFileSync(0, "utf8");
  try {
    return JSON.parse(raw);
  } catch (e) {
    process.stderr.write(`plugin-host: failed to parse manifest JSON: ${e.message}\n`);
    process.exit(2);
  }
}

function buildSourceFile(file) {
  const lineViews = file.lineViews.map(buildLineView);
  return {
    path: file.path,
    text: file.text,
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
    isJsxText: native.isJsxText,
    isPragma: native.isPragma,
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

function resolveOptions(check, perCheckOverrides) {
  const out = {};
  const schema = check.options ?? {};
  const overrides = perCheckOverrides[check.id] ?? {};
  for (const [key, spec] of Object.entries(schema)) {
    out[key] = key in overrides ? overrides[key] : spec.default;
  }
  return out;
}

function fileMatchesScope(absFilePath, scope) {
  if (!scope) return true;
  const exts = scope.extensions;
  if (Array.isArray(exts) && exts.length > 0) {
    const lower = absFilePath.toLowerCase();
    if (!exts.some((e) => lower.endsWith("." + String(e).toLowerCase()))) return false;
  }
  // pathPattern / excludePatterns — treated as always-match in this
  // host. The Rust engine already pre-filters via cd-81a.5's matcher;
  // adding glob matching here would just duplicate work. If a future
  // bead pushes scope filtering into the host, pull globset/picomatch.
  return true;
}
