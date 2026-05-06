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
};

const manifest = readManifest();
const reports = [];
const errors = [];

const loadedPlugins = [];
for (const pluginPath of manifest.plugins) {
  try {
    const pluginDir = resolvePath(manifest.cwd, pluginPath);
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
    if (check.requiresTypes === true) {
      process.stderr.write(
        `[cofferdam] plugin "${check.id}" sets requiresTypes:true — type-aware routing` +
          ` (ts-morph) is not yet wired in 0.2.x; the check will run without type information.` +
          ` Track cd-l58 / gh #16 for status.\n`,
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

// Metadata mode: return check metadata without processing any files.
// Manifest shape: { "mode": "metadata", "cwd": "...", "plugins": [...] }
// Output shape:   {
//   "checks": [{
//     id, category, basePriority, defaultSeverity, explanation, body,
//     requiresTypes, files,
//     options: [{ name, kind, default, doc }]
//   }],
//   "errors": [...]
// }
if (manifest.mode === "metadata") {
  const checks = loadedPlugins.map(({ check }) => ({
    id: check.id,
    category: check.category ?? "warning",
    basePriority: check.basePriority ?? 0,
    defaultSeverity: check.defaultSeverity ?? "medium",
    explanation: check.explanation ?? "",
    body: check.body ?? null,
    requiresTypes: check.requiresTypes ?? false,
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
  process.stdout.write(JSON.stringify({ checks, errors }) + "\n");
  process.exit(0);
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
    if (!fileMatchesScope(file.path, check.files, file.layer ?? null)) continue;

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
        re += "(?:.+/)?"; // zero or more segments with trailing slash
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
