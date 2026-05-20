// SourceFile + CheckContext — the per-file scratch passed to a check's
// `run(file, ctx, opts)` callback. Mirrors cofferdam_core::CheckContext.
// The `corpus` accessor and `FinalizeContext` land in cd-9hp.6 — see
// `PluginCorpus` and `FinalizeContext` below.

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
 * Plugin-side cross-file storage (cd-9hp.6). One `PluginCorpus`
 * instance is created per check per analysis run; the plugin host
 * preserves it across every per-file invocation of that check and
 * hands the same instance to `finalize` if the check declares one.
 *
 * The corpus is plugin-private: two plugins picking the same slot key
 * cannot see each other's data. Internally the host namespaces every
 * key by the calling check's id (cd-9hp.7).
 *
 * ## Type model
 *
 * Slots are addressed by string key and store any JSON-serialisable
 * value. Type drift across reads/writes by the same plugin is the
 * plugin author's problem (TypeScript can't verify across the
 * per-file boundary). Convention: declare a typed alias for each slot
 * and use it consistently:
 *
 * ```ts
 * type ExportRecord = { file: string; name: string; span: Span };
 * // write
 * ctx.corpus.append<ExportRecord>("exports", { file, name, span });
 * // read
 * const all = ctx.corpus.read<ExportRecord[]>("exports") ?? [];
 * ```
 *
 * ## Concurrency
 *
 * The current plugin host is single-threaded — a `run` invocation
 * fully completes before the next file's `run` starts. The corpus
 * accessor does not need locking. If future versions add a
 * worker-pool model, the host owns concurrency control; plugin
 * authors should still treat `read`-then-`write` as non-atomic.
 */
export interface PluginCorpus {
  /**
   * Return the most recent value written to `key`, or `undefined` if
   * the slot was never written by this plugin.
   *
   * The generic is a type hint only — the runtime does no validation;
   * if you wrote a string and read it back as a number, TypeScript
   * cannot save you.
   */
  read<T = unknown>(key: string): T | undefined;

  /**
   * Overwrite the value at `key`. `value` must be JSON-serialisable;
   * the host may snapshot or serialise it across phase boundaries.
   */
  write<T = unknown>(key: string, value: T): void;

  /**
   * Convenience for the common Vec<T> aggregation pattern. Reads the
   * slot, appends `item`, writes back. Initialises the slot to `[]`
   * when missing. Equivalent to:
   *
   * ```ts
   * const arr = ctx.corpus.read<T[]>(key) ?? [];
   * arr.push(item);
   * ctx.corpus.write(key, arr);
   * ```
   */
  append<T = unknown>(key: string, item: T): void;
}

/**
 * Mutable per-file scratch passed to `Check.run`. The check emits
 * findings via `ctx.report(...)`; the engine collects, suppresses,
 * baselines, and renders.
 */
export interface CheckContext {
  /** Emit a finding. Calls accumulate; order is preserved. */
  report(args: ReportArgs): void;

  /**
   * Cross-file storage for this plugin (cd-9hp.6). Populate during
   * per-file `run`; read back in `finalize` to emit findings that
   * depend on the aggregate corpus state.
   */
  readonly corpus: PluginCorpus;
}

/**
 * Per-finding payload passed to {@link FinalizeContext.report}.
 * Extends {@link ReportArgs} with a required `file` since finalize
 * has no implicit "current file" the way per-file `run` does.
 */
export interface FinalizeReportArgs extends ReportArgs {
  /**
   * Absolute file path the finding attaches to. Use the canonical
   * forward-slashed form that the per-file pass saw on `file.path`.
   * Cross-file findings typically also populate `related` with the
   * other implicated locations.
   */
  readonly file: string;
}

/**
 * Context passed to `Check.finalize` (cd-9hp.6) once every per-file
 * `run` has completed. Mirrors `CheckContext` but the file-attached
 * findings emitted here typically describe relationships spanning
 * multiple files (`related` is the natural field for the secondary
 * locations).
 */
export interface FinalizeContext {
  /**
   * Emit a finding. Unlike {@link CheckContext.report}, the args
   * include an explicit `file` — finalize is not scoped to a single
   * file. `related` is the natural place for the secondary locations
   * that motivated the finding.
   */
  report(args: FinalizeReportArgs): void;

  /**
   * The same per-plugin corpus the per-file pass populated. Reads
   * surface every value written during the per-file phase; writes are
   * permitted but rarely useful at this stage.
   */
  readonly corpus: PluginCorpus;
}
