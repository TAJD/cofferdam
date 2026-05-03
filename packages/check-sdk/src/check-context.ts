// SourceFile + CheckContext — the per-file scratch passed to a check's
// `run(file, ctx, opts)` callback. Mirrors cofferdam_core::CheckContext
// minus the bits that are only meaningful in the Rust pipeline (corpus,
// FinalizeContext — those land in their own beads).

import type { AstView } from "./ast.js";
import type { Severity } from "./severity.js";
import type { LineView } from "./line-view.js";
import type { Span } from "./span.js";

/** A source file passed to a check's `run` callback. */
export interface SourceFile {
  /** Absolute path as cofferdam discovered it. Forward-slashed on every host. */
  readonly path: string;
  /** Full text of the file, UTF-8. Byte offsets in spans index into this. */
  readonly text: string;

  /**
   * Iterate every line in the file with classification flags drawn
   * from the comment table + an AST walk over string/template/JSX.
   * See {@link LineView} for flag semantics.
   */
  lines(): IterableIterator<LineView>;

  /**
   * Plugin-facing AST view. `null` only when parsing produced no
   * usable program (the engine emitted `Warning.ParseError` for those
   * files — your check is not invoked again on the failed file).
   */
  readonly ast: AstView | null;
}

/** Per-issue payload passed to {@link CheckContext.report}. */
export interface ReportArgs {
  /** Required. Human-readable problem description. */
  readonly message: string;
  /** Required. Where the issue lives. Build via `LineView.spanFor` or
   *  `SourceFile.ast` node spans. */
  readonly span: Span;
  /**
   * Optional severity override for this specific finding. Defaults to
   * the check's `defaultSeverity` declared on `defineCheck`.
   */
  readonly severity?: Severity;
  /**
   * Optional secondary locations participating in the same finding —
   * e.g. the duplicate of a duplicate-block, the other declarer of a
   * duplicate-export. Omitted from JSON when empty.
   */
  readonly related?: readonly { readonly file: string; readonly span: Span }[];
}

/**
 * Mutable per-file scratch passed to `Check.run`. The check emits
 * findings via `ctx.report(...)`; the engine collects, suppresses,
 * baselines, and renders.
 */
export interface CheckContext {
  /** Emit a finding. Calls accumulate; order is preserved. */
  report(args: ReportArgs): void;
}
