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
      `[host] file=${file.path} lines=${file.lineViews.length} plugins=${loadedPlugins.length}\n`,
    );
  }
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
    ast: null,
  };
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
