// Plugin host runtime — the JS side of cd-81a.7.
//
// Lives in @cofferdam/check-sdk so the loader and the authoring SDK
// share one published surface. The cofferdam binary's JS wrapper (in
// packages/cofferdam) imports `loadPlugin` and `runPlugin` from here
// after it has obtained the native addon's `lineViews` /
// `mergePluginFindings` helpers.
//
// Why worker_threads: a plugin throwing on one file must not crash the
// engine driver, and unbounded plugin runs must be timeout-able.
// worker_threads gives crash containment + per-message timeout for free,
// at the cost of postMessage serialisation per file (cheap relative to
// parsing).

import type { Check, FileScope } from "./define-check.js";
import type {
  CheckContext,
  FinalizeContext,
  FinalizeReportArgs,
  Fix,
  PluginCorpus,
  ReportArgs,
} from "./check-context.js";
import type { LineView } from "./line-view.js";
import type { Span } from "./span.js";
import type { AstNode, AstView, AstVisitor, NodeKind, Walk } from "./ast.js";

// ---- shapes the napi addon hands us ---------------------------------

export interface NativeLineView {
  readonly lineNo: number;
  readonly text: string;
  readonly isComment: boolean;
  readonly isDocComment: boolean;
  readonly isStringLiteral: boolean;
  readonly stringLiteralRanges?: ReadonlyArray<readonly [number, number]>;
  readonly isJsxText: boolean;
  readonly isPragma: boolean;
  readonly isTag: boolean;
  readonly isText: boolean;
  readonly lineStart: number;
}

/**
 * One report that crosses the worker_thread boundary back to the
 * loader. Mirrors the napi `JsReport` struct in cofferdam-napi.
 */
export interface PluginReport {
  readonly checkId: string;
  readonly message: string;
  readonly file: string;
  readonly startByte: number;
  readonly endByte: number;
  readonly severity: string;
  readonly fix?: Fix;
  readonly related?: readonly { readonly file: string; readonly span: Span }[];
}

/**
 * Runtime input passed from the loader to a plugin's `run()` callback.
 * Built from native `lineViews()` + the file path/text. Cheap to
 * serialise (no AST handles cross the boundary).
 */
export interface PluginRunInput {
  readonly path: string;
  readonly text: string;
  readonly lineViews: readonly NativeLineView[];
  /** Layer name from `cofferdam.invariants.toml` `[layers]`. `null` when not in any layer. */
  readonly layer: string | null;
  /**
   * Flat-array AST wire payload (see `design/sdk-ast-wire.md` and
   * `crates/cofferdam-cli/src/ast_wire.rs`). `undefined`/`null` when the
   * caller has no parsed AST for this file (e.g. a non-JS/TS source).
   */
  readonly ast?: AstWireInput | null;
}

/** One flat-array node in the AST wire payload. Per-kind extras are
 * flattened onto the object (see `WireExtras` in `ast_wire.rs`). */
export interface AstWireNode {
  readonly kind: NodeKind;
  readonly span: Span;
  readonly firstChild: number;
  readonly nextSibling: number;
  readonly [extra: string]: unknown;
}

export interface AstWireInput {
  readonly rootIdx: number;
  readonly nodes: readonly AstWireNode[];
}

// ---- runtime construction --------------------------------------------

function buildLineView(native: NativeLineView): LineView {
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
    spanFor(charStart: number, charEnd: number): Span {
      return {
        line: native.lineNo,
        column: charStart + 1,
        start_byte: native.lineStart + charStart,
        end_byte: native.lineStart + charEnd,
      };
    },
  };
}

// Map from wire-side AST node kind to the visitor method name plugins
// implement. Kept in sync by hand with `VISITOR_METHODS` in
// `crates/cofferdam-cli/scripts/plugin-host.mjs` — that script is the
// production NDJSON-subprocess host and is deliberately self-contained
// (no import of this package), so the two copies can't share source.
const VISITOR_METHODS: Partial<Record<NodeKind, keyof AstVisitor>> = {
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

/**
 * Reconstruct a typed {@link AstView} from the flat-array wire payload
 * (see `design/sdk-ast-wire.md`). Mirrors `buildAstView` in
 * `crates/cofferdam-cli/scripts/plugin-host.mjs` — both must be kept in
 * sync when a new `NodeKind` is added.
 */
// Per-kind population of the flat `out` object for `buildAstView`'s
// `get()`. Split out of `get()` itself (formerly one 137-line switch,
// cyclomatic complexity 30) so each kind's shape is a small, independently
// readable function; `get()` is left as just cache-check + dispatch.
type NodeBuilder = (
  w: AstWireNode,
  out: Record<string, unknown>,
  idx: number,
  ctx: { get: (idx: number) => AstNode | null; collectChildren: (idx: number) => AstNode[] },
) => void;

const NODE_BUILDERS: Partial<Record<NodeKind, NodeBuilder>> = {
  Program: (_w, out, idx, ctx) => {
    Object.defineProperty(out, "body", { enumerable: true, get: () => ctx.collectChildren(idx) });
  },
  CallExpression: (w, out, _idx, ctx) => {
    Object.defineProperty(out, "callee", { enumerable: true, get: () => ctx.get(w["calleeIdx"] as number) });
    Object.defineProperty(out, "arguments", {
      enumerable: true,
      get: () => ((w["argumentIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
  },
  ImportDeclaration: (w, out) => {
    out["source"] = w["source"];
    out["specifiers"] = w["specifiers"] ?? [];
  },
  Function: (w, out, _idx, ctx) => {
    out["name"] = w["name"] ?? undefined;
    out["async"] = !!w["async"];
    out["generator"] = !!w["generator"];
    Object.defineProperty(out, "params", {
      enumerable: true,
      get: () => ((w["paramIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
  },
  ArrowFunctionExpression: (w, out, _idx, ctx) => {
    out["async"] = !!w["async"];
    out["expression"] = !!w["expression"];
    Object.defineProperty(out, "params", {
      enumerable: true,
      get: () => ((w["paramIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
  },
  Class: (w, out) => {
    out["name"] = w["name"] ?? undefined;
  },
  ObjectExpression: (w, out, _idx, ctx) => {
    Object.defineProperty(out, "properties", {
      enumerable: true,
      get: () => ((w["propertyIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
  },
  MemberExpression: (w, out, _idx, ctx) => {
    out["property"] = w["property"] ?? undefined;
    out["computed"] = !!w["computed"];
    Object.defineProperty(out, "object", { enumerable: true, get: () => ctx.get(w["objectIdx"] as number) });
  },
  IdentifierReference: (w, out) => {
    out["name"] = w["name"];
  },
  VariableDeclaration: (w, out, _idx, ctx) => {
    out["declarationKind"] = w["declarationKind"];
    const decls =
      (w["declarations"] as { name?: string; initIdx: number; initSpan?: Span }[]) ?? [];
    out["declarations"] = decls.map((d) => ({
      name: d.name ?? undefined,
      init: ctx.get(d.initIdx ?? -1) ?? undefined,
      initSpan: d.initSpan ?? undefined,
    }));
  },
  JSXElement: (w, out, _idx, ctx) => {
    out["tagName"] = w["tagName"];
    out["selfClosing"] = !!w["selfClosing"];
    Object.defineProperty(out, "attributes", {
      enumerable: true,
      get: () => ((w["attributeIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
  },
  JSXAttribute: (w, out) => {
    out["name"] = w["name"] ?? undefined;
    out["value"] = w["value"] ?? undefined;
    out["isExpression"] = !!w["isExpression"];
    out["isSpread"] = !!w["isSpread"];
  },
  Document: (_w, out, idx, ctx) => {
    Object.defineProperty(out, "children", {
      enumerable: true,
      get: () => ctx.collectChildren(idx).filter((n) => n.kind !== "Attribute"),
    });
  },
  Element: (w, out, idx, ctx) => {
    out["tagName"] = w["tagName"];
    out["selfClosing"] = !!w["selfClosing"];
    Object.defineProperty(out, "attributes", {
      enumerable: true,
      get: () => ((w["attributeIdxs"] as number[]) ?? []).map(ctx.get).filter((n) => n !== null),
    });
    Object.defineProperty(out, "children", {
      enumerable: true,
      get: () => ctx.collectChildren(idx).filter((n) => n.kind !== "Attribute"),
    });
  },
  Attribute: (w, out) => {
    out["name"] = w["name"] ?? undefined;
    out["value"] = w["value"] ?? undefined;
  },
  Text: (w, out) => {
    out["text"] = w["text"];
  },
  Comment: (w, out) => {
    out["text"] = w["text"];
  },
  Doctype: (w, out) => {
    out["text"] = w["text"];
  },
};

export function buildAstView(wire: AstWireInput): AstView {
  const { rootIdx, nodes } = wire;
  const built = new Array<AstNode | null>(nodes.length).fill(null);

  function get(idx: number): AstNode | null {
    if (idx < 0 || idx >= nodes.length) return null;
    const cached = built[idx];
    if (cached) return cached;
    const w = nodes[idx]!;
    const out: Record<string, unknown> = { kind: w.kind, span: w.span };
    NODE_BUILDERS[w.kind]?.(w, out, idx, { get, collectChildren });
    const node = out as unknown as AstNode;
    built[idx] = node;
    return node;
  }

  function collectChildren(idx: number): AstNode[] {
    const out: AstNode[] = [];
    let cursor = nodes[idx]?.firstChild ?? -1;
    while (cursor >= 0) {
      const node = get(cursor);
      if (node) out.push(node);
      cursor = nodes[cursor]!.nextSibling;
    }
    return out;
  }

  const indexByKind = new Map<NodeKind, number[]>();
  for (let i = 0; i < nodes.length; i++) {
    const k = nodes[i]!.kind;
    let idxs = indexByKind.get(k);
    if (!idxs) {
      idxs = [];
      indexByKind.set(k, idxs);
    }
    idxs.push(i);
  }

  return {
    get root(): AstNode {
      // rootIdx is always 0 (the Program node) for any non-empty wire.
      return get(rootIdx) as AstNode;
    },
    findAll<K extends NodeKind>(kind: K) {
      const idxs = indexByKind.get(kind) ?? [];
      return idxs.map(get) as never;
    },
    walk(visitor: AstVisitor): void {
      const visit = (idx: number): void => {
        if (idx < 0) return;
        const w = nodes[idx]!;
        const methodName = VISITOR_METHODS[w.kind];
        const fn = methodName ? visitor[methodName] : undefined;
        const node = get(idx);
        const decision: Walk = fn && node ? (fn as (n: AstNode) => Walk).call(visitor, node) : "continue";
        if (decision !== "skip" && w.firstChild >= 0) visit(w.firstChild);
        visit(w.nextSibling);
      };
      visit(rootIdx);
    },
  };
}

/**
 * Build the `SourceFile` shape a plugin's `run()` callback expects.
 * `ast` is `null` when `input.ast` is absent — full AST access requires
 * the caller (loader / test harness) to supply the wire payload.
 */
export function buildSourceFile(input: PluginRunInput) {
  const lineViews = input.lineViews.map(buildLineView);
  return {
    path: input.path,
    text: input.text,
    layer: input.layer,
    lines(): IterableIterator<LineView> {
      let i = 0;
      const it: IterableIterator<LineView> = {
        next(): IteratorResult<LineView> {
          if (i < lineViews.length) {
            return { value: lineViews[i++]!, done: false };
          }
          return { value: undefined as unknown as LineView, done: true };
        },
        [Symbol.iterator](): IterableIterator<LineView> {
          return it;
        },
        return(value?: LineView): IteratorResult<LineView> {
          i = lineViews.length;
          return { value: value as LineView, done: true };
        },
      };
      return it;
    },
    ast: input.ast ? buildAstView(input.ast) : null,
  };
}

// ---- runPlugin -------------------------------------------------------

/**
 * Return true when `path` (normalised to forward slashes) is in scope
 * for the given `FileScope` filter. Mirrors the `fileMatchesScope`
 * logic in `plugin-host.mjs` — both must be kept in sync.
 *
 * `undefined` scope = every file is in scope. `layer` is the file's
 * resolved layer (or `null` when the file is outside every declared
 * layer); used to filter against `scope.layers` when present.
 */
export function fileMatchesScope(
  path: string,
  scope: FileScope | undefined,
  layer: string | null = null,
): boolean {
  if (!scope) return true;

  const fwd = path.replace(/\\/g, "/");

  const { extensions } = scope;
  if (Array.isArray(extensions) && extensions.length > 0) {
    const lower = fwd.toLowerCase();
    if (!extensions.some((e: string) => lower.endsWith("." + String(e).toLowerCase()))) {
      return false;
    }
  }

  const { layers } = scope;
  if (Array.isArray(layers) && layers.length > 0) {
    if (layer === null || !layers.includes(layer)) return false;
  }

  const includes: string[] = [];
  if (typeof scope.pathPattern === "string" && scope.pathPattern) {
    includes.push(scope.pathPattern);
  }
  if (Array.isArray(scope.pathPatterns)) {
    for (const p of scope.pathPatterns) {
      if (typeof p === "string" && p) includes.push(p);
    }
  }

  if (includes.length > 0 && !includes.some((pat) => globMatch(pat, fwd))) return false;

  const { excludePatterns } = scope;
  if (Array.isArray(excludePatterns) && excludePatterns.length > 0) {
    if (excludePatterns.some((pat: string) => typeof pat === "string" && globMatch(pat, fwd))) {
      return false;
    }
  }

  return true;
}

function globMatch(pattern: string, path: string): boolean {
  const braceMatch = pattern.match(/^(.*?)\{([^{}]+)\}(.*)$/);
  if (braceMatch) {
    const [, pre, inner, post] = braceMatch;
    return (inner as string).split(",").some((alt) => globMatch((pre as string) + alt + (post as string), path));
  }
  // For relative patterns (no leading `/` or `**/`), also try `**/pattern`.
  if (!pattern.startsWith("/") && !pattern.startsWith("**/")) {
    if (globMatchSingle("**/" + pattern, path)) return true;
  }
  return globMatchSingle(pattern, path);
}

function globMatchSingle(pattern: string, path: string): boolean {
  let re = "^";
  let i = 0;
  while (i < pattern.length) {
    const c = pattern[i]!;
    if (c === "*") {
      if (pattern[i + 1] === "*") {
        i += 2;
        if (pattern[i] === "/") i++;
        re += "(?:.+/)?";
        continue;
      }
      re += "[^/]*";
    } else if (c === "?") {
      re += "[^/]";
    } else if (c === "[") {
      const end = pattern.indexOf("]", i + 1);
      if (end === -1) {
        re += "\\[";
      } else {
        re += pattern.slice(i, end + 1);
        i = end;
      }
    } else {
      re += c.replace(/[.+^${}()|\\]/g, "\\$&");
    }
    i++;
  }
  re += "$";
  try {
    return new RegExp(re).test(path);
  } catch {
    return false;
  }
}

/**
 * In-memory backing for {@link PluginCorpus}. The plugin host creates
 * one per check per analysis run and reuses it across every per-file
 * `runPlugin` call (and the optional `runPluginFinalize` call) so the
 * slots survive the per-file boundary.
 *
 * Plugins never see this directly — they see the `PluginCorpus`
 * accessor built from it. Exported because the loader script needs
 * to instantiate one when wiring multi-file runs together.
 */
export class PluginCorpusStore {
  private readonly slots = new Map<string, unknown>();

  /** Accessor view that satisfies {@link PluginCorpus}. */
  public view(): PluginCorpus {
    const slots = this.slots;
    return {
      read<T = unknown>(key: string): T | undefined {
        return slots.get(key) as T | undefined;
      },
      write<T = unknown>(key: string, value: T): void {
        slots.set(key, value);
      },
      append<T = unknown>(key: string, item: T): void {
        const existing = slots.get(key);
        if (Array.isArray(existing)) {
          (existing as T[]).push(item);
        } else {
          slots.set(key, [item]);
        }
      },
    };
  }
}

/**
 * Execute a single plugin against a single file. Collects `ctx.report`
 * calls into a `PluginReport[]` the loader can hand to native
 * `mergePluginFindings`.
 *
 * This is the function a worker_thread invokes. The host process never
 * touches plugin code directly — crash containment is the worker's
 * boundary.
 *
 * FileScope filtering is applied here: if the check declares a `files`
 * scope and the input path does not match, the function returns an empty
 * array immediately without calling `run()`.
 *
 * Pass `corpus` to preserve slot state across the multi-file run. When
 * omitted, a fresh `PluginCorpusStore` is built for this single call —
 * fine for one-shot tests but useless for cross-file aggregation.
 */
export function runPlugin(
  check: Check,
  input: PluginRunInput,
  corpus?: PluginCorpusStore,
): PluginReport[] {
  if (!fileMatchesScope(input.path, check.files, input.layer)) return [];

  const reports: PluginReport[] = [];
  const corpusView = (corpus ?? new PluginCorpusStore()).view();
  const ctx: CheckContext = {
    report(args: ReportArgs): void {
      reports.push(buildReport(check, input.path, args));
    },
    corpus: corpusView,
  };

  // Resolve options — currently use the schema defaults; the loader can
  // pass user-supplied overrides through cofferdam.toml in a follow-up.
  // The cast is sound because the schema's defaults already satisfy
  // ResolvedOptions<S>'s mapped types by construction.
  const opts = resolveDefaults(check.options);

  try {
    check.run(buildSourceFile(input), ctx, opts as Parameters<typeof check.run>[2]);
  } catch (err) {
    // Don't swallow the plugin error — push a synthetic report so the
    // engine surfaces it as `Warning.PluginCrashed` and analysis
    // continues for the rest of the corpus.
    reports.push({
      checkId: "Warning.PluginCrashed",
      message: `plugin '${check.id}' threw: ${err instanceof Error ? err.message : String(err)}`,
      file: input.path,
      startByte: 0,
      endByte: 0,
      severity: "high",
    });
  }

  return reports;
}

/**
 * Run a check's optional `finalize` hook after every per-file `run`
 * has completed (cd-9hp.6). Returns the additional reports the check
 * emitted at this stage; an empty array when the check declares no
 * finalize. Reuses `corpus` so finalize sees the slots the per-file
 * pass populated.
 *
 * A finalize crash is surfaced as a `Warning.PluginCrashed` report
 * with `file = ""` and zero spans, mirroring how `runPlugin` handles a
 * per-file throw.
 */
export function runPluginFinalize(
  check: Check,
  corpus: PluginCorpusStore,
): PluginReport[] {
  if (typeof check.finalize !== "function") return [];

  const reports: PluginReport[] = [];
  const ctx: FinalizeContext = {
    report(args: FinalizeReportArgs): void {
      reports.push(buildReport(check, args.file, args));
    },
    corpus: corpus.view(),
  };

  const opts = resolveDefaults(check.options);

  try {
    check.finalize(ctx, opts as Parameters<NonNullable<typeof check.finalize>>[1]);
  } catch (err) {
    reports.push({
      checkId: "Warning.PluginCrashed",
      message: `plugin '${check.id}' finalize threw: ${err instanceof Error ? err.message : String(err)}`,
      file: "",
      startByte: 0,
      endByte: 0,
      severity: "high",
    });
  }

  return reports;
}

function buildReport(check: Check, defaultFile: string, args: ReportArgs): PluginReport {
  const { span, severity, related, fix } = args;
  return {
    checkId: check.id,
    message: args.message,
    file: defaultFile,
    startByte: span.start_byte,
    endByte: span.end_byte,
    severity: severity ?? check.defaultSeverity,
    ...(fix !== undefined ? { fix } : {}),
    ...(related !== undefined ? { related } : {}),
  };
}

function resolveDefaults<S extends Record<string, { readonly default: unknown }>>(
  schema: S,
): { readonly [K in keyof S]: S[K]["default"] } {
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(schema) as (keyof S)[]) {
    out[key as string] = schema[key]!.default;
  }
  return out as { readonly [K in keyof S]: S[K]["default"] };
}
