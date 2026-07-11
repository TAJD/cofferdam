// Shared ts-morph type resolution logic (CD-81).
//
// Extracted from type-host.mjs so both the standalone type-host worker
// (crates/cofferdam-cli/src/type_host.rs, wire protocol in
// design/type-host-wire.md) and the in-process plugin host
// (crates/cofferdam-cli/scripts/plugin-host.mjs) can resolve types
// through the exact same ts-morph plumbing without a second RPC hop.
//
// Every function here takes an explicit `state` object instead of
// closing over module-level state, so each host script can hold its
// own independent cache even though both run in the same process model
// (one Node process per host). Use `createTypeHostState()` to build a
// fresh one.

import { readFileSync, statSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { dirname, join as joinPath, resolve as resolvePath } from "node:path";

// Fresh per-host cache: the ts-morph module (loaded once) and any
// opened ts-morph Projects (cached by tsconfig path).
export function createTypeHostState() {
  return {
    tsMorph: null, // null = not loaded, object = loaded module
    tsMorphVersion: null, // string | null
    tsMorphLoadError: null, // string | null
    projects: new Map(), // tsconfigPath -> ts-morph Project
  };
}

// Load (or return the cached) ts-morph module into `state`. Returns the
// import wall-clock time in ms, or 0 if already loaded/failed. Node ESM
// ignores NODE_PATH, so ts-morph is resolved by hand: walk up from
// `projectRoot` looking for `node_modules/ts-morph/package.json`, then
// construct a file:// URL to the package's main entry and import that.
export async function ensureTsMorphLoaded(state, projectRoot) {
  if (state.tsMorph) return 0;
  if (state.tsMorphLoadError) return 0; // cached failure; caller will throw

  const importStart = nowMs();
  try {
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

// Open (or return the cached) ts-morph Project for a tsconfig path.
// Returns { project, initMs, cached }. Throws a `ts_morph_unavailable`
// or `project_init_failed` error (mirroring the type-host wire codes)
// on failure.
export async function getOrCreateProject(state, tsconfigPath) {
  // Callers that already loaded ts-morph against a known project root
  // (the plugin host, up front in the header handler) pay nothing here.
  // Callers that haven't (the standalone type-host worker, which loads
  // lazily per-request) fall back to the tsconfig's own directory as the
  // `node_modules` search root — the same directory `type_host.rs`
  // already passes as `COFFERDAM_TYPE_HOST_PROJECT_ROOT`.
  await ensureTsMorphLoaded(state, dirname(tsconfigPath));
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
export function resolveSourceFile(project, file) {
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

// Resolve the type of the node at a byte span.
// Returns TypeFacts { text, isNullable, includesNull, includesUndefined,
// isAny } or null when no meaningful type could be resolved. Throws a
// `ts_morph_unavailable` / `project_init_failed` error (see
// `getOrCreateProject`) on failure to open ts-morph/the project — a
// `null` return means "resolved fine, nothing found here", distinct
// from a thrown error meaning "couldn't even try".
export async function typeAt(state, tsconfigPath, file, startByte, endByte) {
  const { project } = await getOrCreateProject(state, tsconfigPath);

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

// Resolve an identifier/import reference to a literal value (CD-82).
// Follows ts-morph symbol resolution across files — an imported binding's
// symbol is aliased to the symbol at its origin declaration, so
// `import { x } from "./constants"` resolves through to `const x = "..."`
// in constants.ts as long as both files are part of the open Project.
//
// Returns LiteralFacts { literalString?, isNullable, isEmptyObject } or
// `null` when the node/symbol can't be resolved at all (no identifier at
// that span, no symbol, no declarations). A resolved-but-uninformative
// declaration (e.g. a function call initializer) still returns facts —
// `literalString: undefined` plus whatever nullability the declared/
// inferred type carries — same "best-effort facts, null only when truly
// nothing resolvable" philosophy as `typeAt`.
export async function resolveLiteral(state, tsconfigPath, file, startByte, endByte) {
  const { project } = await getOrCreateProject(state, tsconfigPath);

  const sourceFile = resolveSourceFile(project, file);
  if (!sourceFile) return null;

  const fullText = sourceFile.getFullText();
  const startU16 = utf16PosFromByteOffset(fullText, startByte | 0);
  const endU16 = utf16PosFromByteOffset(fullText, endByte | 0);

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

  const { Node, SyntaxKind } = state.tsMorph;

  const idNode = Node.isIdentifier(node)
    ? node
    : (safeCall(() => node.getFirstDescendantByKind(SyntaxKind.Identifier), null) ?? null);
  if (!idNode) return null;

  let symbol = safeCall(() => idNode.getSymbol(), null);
  if (!symbol) return null;
  const aliased = safeCall(() => symbol.getAliasedSymbol(), null);
  if (aliased) symbol = aliased;

  const declarations = safeCall(() => symbol.getDeclarations(), []) ?? [];
  if (declarations.length === 0) return null;

  const target = declarations.find((d) => Node.isVariableDeclaration(d)) ?? declarations[0];
  return literalFactsFromDeclaration(target, Node, SyntaxKind);
}

// Build LiteralFacts from a declaration node (typically a
// VariableDeclaration reached via symbol resolution in `resolveLiteral`).
function literalFactsFromDeclaration(declNode, Node, SyntaxKind) {
  let literalString;
  let isNullable = false;
  let isEmptyObject = false;

  const initializer = safeCall(() => declNode.getInitializer?.(), null);
  if (initializer) {
    if (Node.isStringLiteral(initializer) || Node.isNoSubstitutionTemplateLiteral(initializer)) {
      literalString = safeCall(() => initializer.getLiteralValue(), undefined);
    } else if (initializer.getKind() === SyntaxKind.NullKeyword) {
      isNullable = true;
    } else if (Node.isIdentifier(initializer) && initializer.getText() === "undefined") {
      isNullable = true;
    } else if (
      Node.isObjectLiteralExpression(initializer) &&
      initializer.getProperties().length === 0
    ) {
      isEmptyObject = true;
    }
  }

  // Fold in declared/inferred type nullability regardless of what the
  // initializer shape told us — a `let` with no initializer, or a type
  // annotation wider than the initializer, still contributes knowable
  // facts.
  const type = safeCall(() => declNode.getType(), null);
  if (type) {
    const unionParts = safeCall(() => type.getUnionTypes(), []) ?? [];
    const parts = unionParts.length > 0 ? unionParts : [type];
    const includesNull = parts.some((t) => safeCall(() => t.isNull(), false));
    const includesUndefined = parts.some((t) => safeCall(() => t.isUndefined(), false));
    if (includesNull || includesUndefined) isNullable = true;
  }

  return { literalString, isNullable, isEmptyObject };
}

// Build the compact TypeFacts wire object from a ts-morph Type.
export function typeFacts(type, node) {
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

export function safeCall(fn, fallback) {
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
export function utf16PosFromByteOffset(text, byteOffset) {
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

// Walk up from `startDir`, looking for `node_modules/ts-morph/package.json`.
// Returns `{ entryPath, version }` on success — entryPath is the absolute
// path to the package's main file (or its `main`/`exports["."]` entry),
// suitable for a `pathToFileURL(...).href` import.
export function resolveTsMorph(startDir) {
  let dir = resolvePath(startDir);
  for (let depth = 0; depth < 32; depth++) {
    const pkgPath = joinPath(dir, "node_modules", "ts-morph", "package.json");
    // CD-92: every non-success path below must fall through to the
    // parent-dir advance at the bottom of the loop — a bare `continue`
    // here re-checks the *same* `dir` next iteration instead of climbing,
    // so a directory with a broken/unresolvable ts-morph install could
    // exhaust the whole 32-iteration budget without ever reaching a
    // working install further up the tree.
    let resolved = null;
    try {
      const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
      const pkgDir = dirname(pkgPath);
      const entry = pickEntry(pkg);
      const entryPath = entry
        ? joinPath(pkgDir, entry)
        : findIndexFallback(pkgDir);
      if (entryPath) {
        try {
          statSync(entryPath);
          resolved = {
            entryPath,
            version: typeof pkg.version === "string" ? pkg.version : null,
          };
        } catch {
          // entry file doesn't actually exist — fall through to parent
        }
      }
    } catch {
      // no ts-morph package.json here — fall through to parent
    }
    if (resolved) return resolved;

    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

export function pickEntry(pkg) {
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

export function findIndexFallback(pkgDir) {
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
