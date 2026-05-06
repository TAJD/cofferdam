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
   * Layer name from `cofferdam.invariants.toml` `[layers]`.
   * `null` when the file is not a member of any declared layer.
   */
  readonly layer: string | null;

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

/**
 * A single mechanical fix: replace the bytes in `span` with `replacement`.
 * Mirrors `cofferdam_core::TextEdit`. Plugin authors attach this at
 * report time; the cofferdam fix engine prefers it over the built-in
 * `Check::autofix` trait method (cd-81a.6).
 *
 * Edits must be non-overlapping; the fix engine applies them in reverse
 * byte-offset order so earlier replacements don't invalidate later spans.
 */
export interface Fix {
  readonly span: Span;
  readonly replacement: string;
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
  /**
   * Optional autofix payload. When present, `cofferdam fix` applies it
   * in place of any built-in autofix logic for this check.
   */
  readonly fix?: Fix;
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
