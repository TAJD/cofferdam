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

import type { Check } from "./define-check.js";
import type { CheckContext, Fix, ReportArgs } from "./check-context.js";
import type { LineView } from "./line-view.js";
import type { Span } from "./span.js";

// ---- shapes the napi addon hands us ---------------------------------

export interface NativeLineView {
  readonly lineNo: number;
  readonly text: string;
  readonly isComment: boolean;
  readonly isDocComment: boolean;
  readonly isStringLiteral: boolean;
  readonly isJsxText: boolean;
  readonly isPragma: boolean;
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
}

// ---- runtime construction --------------------------------------------

function buildLineView(native: NativeLineView): LineView {
  return {
    lineNo: native.lineNo,
    text: native.text,
    isComment: native.isComment,
    isDocComment: native.isDocComment,
    isStringLiteral: native.isStringLiteral,
    isJsxText: native.isJsxText,
    isPragma: native.isPragma,
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

/**
 * Build the `SourceFile` shape a plugin's `run()` callback expects.
 * `ast` is `null` here — AST access lives behind a separate napi call
 * the loader can route through worker_threads in a follow-up bead. The
 * line-walk Pattern A checks (BrandCasing) work today.
 */
export function buildSourceFile(input: PluginRunInput) {
  const lineViews = input.lineViews.map(buildLineView);
  return {
    path: input.path,
    text: input.text,
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
    ast: null,
  };
}

// ---- runPlugin -------------------------------------------------------

/**
 * Execute a single plugin against a single file. Collects `ctx.report`
 * calls into a `PluginReport[]` the loader can hand to native
 * `mergePluginFindings`.
 *
 * This is the function a worker_thread invokes. The host process never
 * touches plugin code directly — crash containment is the worker's
 * boundary.
 */
export function runPlugin(check: Check, input: PluginRunInput): PluginReport[] {
  const reports: PluginReport[] = [];
  const ctx: CheckContext = {
    report(args: ReportArgs): void {
      const { span, severity, related, fix } = args;
      const report: PluginReport = {
        checkId: check.id,
        message: args.message,
        file: input.path,
        startByte: span.start_byte,
        endByte: span.end_byte,
        severity: severity ?? check.defaultSeverity,
        ...(fix !== undefined ? { fix } : {}),
        ...(related !== undefined ? { related } : {}),
      };
      reports.push(report);
    },
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

function resolveDefaults<S extends Record<string, { readonly default: unknown }>>(
  schema: S,
): { readonly [K in keyof S]: S[K]["default"] } {
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(schema) as (keyof S)[]) {
    out[key as string] = schema[key]!.default;
  }
  return out as { readonly [K in keyof S]: S[K]["default"] };
}
