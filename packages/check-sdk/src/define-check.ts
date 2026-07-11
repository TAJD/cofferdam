// defineCheck — the factory plugin authors call to declare a check.
//
// Type inference flows from the `options` schema declared on the input
// to the `opts` argument of `run`. This is the surface that earns the
// SDK its right to exist — without it, plugin authors hand-write
// matching types twice.

import type { Category } from "./category.js";
import type {
  CheckContext,
  FinalizeContext,
  SourceFile,
} from "./check-context.js";
import type { OptionsSchema, ResolvedOptions } from "./options.js";
import type { Severity } from "./severity.js";

/**
 * Declarative file-scope filter (cd-81a.5). When set, the engine
 * pre-filters discovered files against this scope before invoking
 * `run()`. Replaces the `scannable?` boilerplate that examplco-host
 * checks ship today.
 */
export interface FileScope {
  /** File extensions (without leading dot). Empty/omitted = any. */
  readonly extensions?: readonly string[];
  /**
   * Optional layer allowlist. When set, the file's layer (resolved from
   * `cofferdam.invariants.toml` / `cofferdam.toml` `[layers]`) must be in
   * this set. Files outside any declared layer (`file.layer === null`)
   * never match a non-empty `layers` filter — declare a layer for them
   * if you need them in scope.
   *
   * Empty/omitted = no layer filter (any layer or no layer matches).
   * Reuses the same `file.layer` field plugins can read imperatively.
   */
  readonly layers?: readonly string[];
  /**
   * Optional include glob (gitignore syntax via globset). When set,
   * the file path must match.
   *
   * @deprecated Prefer {@link FileScope.pathPatterns} (plural) for new
   *   code — it accepts an array, matching the shape of `excludePatterns`
   *   and avoiding brace expansion in a single string. `pathPattern`
   *   remains functional through the 0.2.x line and is scheduled for
   *   removal in 0.3.0. When both forms are present, a file matches the
   *   include set if it matches `pathPattern` OR any `pathPatterns`
   *   entry.
   */
  readonly pathPattern?: string;
  /**
   * Optional include globs (gitignore syntax via globset). When set,
   * the file path must match at least one entry. Plural counterpart to
   * the deprecated singular {@link FileScope.pathPattern}; when both are
   * present, a file is in scope if it matches either form.
   */
  readonly pathPatterns?: readonly string[];
  /**
   * Exclude globs. Paths matching any of these are skipped, even if
   * `pathPattern` / `pathPatterns` matched.
   */
  readonly excludePatterns?: readonly string[];
}

/**
 * The shape `defineCheck` returns — what the napi loader picks up
 * from the plugin module's default export. Holds the static metadata
 * + the `run` callback.
 *
 * The `_optionsKeyType` field is a phantom (never set at runtime); it
 * pins the schema shape into the type so the loader can pass typed
 * `opts` without TS losing inference.
 */
export interface Check<S extends OptionsSchema = OptionsSchema> {
  readonly id: string;
  readonly category: Category;
  readonly basePriority: number;
  readonly defaultSeverity: Severity;
  readonly explanation: string;
  /** Optional long-form catalog body used by `cofferdam explain --full`. */
  readonly body: string | undefined;
  /**
   * Whether the check needs full type-aware analysis (routes through
   * ts-morph). When `true`, the plugin host resolves types in-process
   * via ts-morph (CD-81) and exposes `ctx.types.typeAt(startByte,
   * endByte)` inside `run()` — see {@link TypeQuery}. `ctx.types` is
   * `undefined` when no tsconfig was found or ts-morph couldn't load;
   * guard with `if (ctx.types)` rather than assuming presence.
   */
  readonly requiresTypes: boolean;
  /**
   * Whether this check is eligible to run against build-output files
   * discovered by `cofferdam verify --dist` (CD-85). Defaults to
   * `false` — a check must opt in explicitly to run in output mode. A
   * check with `outputMode: true` still runs normally under `cofferdam
   * check` too; this flag only gates eligibility for `verify`.
   */
  readonly outputMode: boolean;
  /** Two-pass consistency mode. */
  readonly consistency: boolean;
  readonly options: S;
  /** File-scope filter. `undefined` = applies to every file. */
  readonly files: FileScope | undefined;
  /**
   * May return a `Promise` — the plugin host awaits `run()`, so a check
   * that needs to `await ctx.types.typeAt(...)` can declare `async
   * run(...)`.
   */
  run(file: SourceFile, ctx: CheckContext, opts: ResolvedOptions<S>): void | Promise<void>;
  /**
   * Optional cross-file finalize hook (cd-9hp.6). Invoked once per
   * analysis run after every file's `run` has completed, in plugin
   * declaration order. Use this to emit findings that depend on
   * corpus-wide state populated during the per-file pass — orphan
   * exports, duplicate names, layered-import audits.
   *
   * Findings emitted here flow through the same suppression, baseline,
   * and severity machinery as per-file findings.
   *
   * May return a `Promise` — the plugin host awaits `finalize()` (CD-91),
   * so a hook that needs to `await ctx.types.typeAt(...)` or do async
   * corpus work can declare `async finalize(...)`.
   */
  finalize?(ctx: FinalizeContext, opts: ResolvedOptions<S>): void | Promise<void>;
}

/**
 * Input to `defineCheck`. Same shape as {@link Check} but the schema
 * generic is inferred at the call site. Optional fields may be omitted
 * (TS picks the conservative default).
 */
export interface DefineCheckInput<S extends OptionsSchema> {
  readonly id: string;
  readonly category: Category;
  readonly basePriority: number;
  /** Defaults to `Severity.Medium` if omitted. */
  readonly defaultSeverity?: Severity;
  readonly explanation: string;
  readonly body?: string;
  /**
   * Whether the check needs full type-aware analysis (routes through
   * ts-morph). When `true`, the plugin host resolves types in-process
   * via ts-morph (CD-81) and exposes `ctx.types.typeAt(startByte,
   * endByte)` inside `run()`. `ctx.types` is `undefined` when no
   * tsconfig was found or ts-morph couldn't load — guard with `if
   * (ctx.types)` rather than assuming presence.
   */
  readonly requiresTypes?: boolean;
  /**
   * Whether this check is eligible to run against build-output files
   * discovered by `cofferdam verify --dist` (CD-85). Defaults to
   * `false`.
   */
  readonly outputMode?: boolean;
  readonly consistency?: boolean;
  /** Schema; defaults to `{}`. Inferred at the call site. */
  readonly options?: S;
  /** File-scope filter; defaults to "every file". */
  readonly files?: FileScope;
  /**
   * May return a `Promise` — the plugin host awaits `run()`, so a check
   * that needs to `await ctx.types.typeAt(...)` can declare `async
   * run(...)`.
   */
  run(file: SourceFile, ctx: CheckContext, opts: ResolvedOptions<S>): void | Promise<void>;
  /**
   * Optional cross-file finalize hook (cd-9hp.6). See the
   * {@link Check.finalize} doc on the output interface.
   */
  finalize?(ctx: FinalizeContext, opts: ResolvedOptions<S>): void | Promise<void>;
}

/**
 * Author a cofferdam plugin check. The call site is the contract: the
 * value passed in is the value the napi loader receives, with TS
 * inference flowing from `options` into `run`'s third argument.
 *
 * @example
 *   import { defineCheck, Category, Severity } from "@cofferdam/check-sdk";
 *
 *   export default defineCheck({
 *     id: "BrandCasing",
 *     category: Category.Warning,
 *     basePriority: 15,
 *     explanation: "Brand name must be all-caps in user-facing copy.",
 *     options: {
 *       brand: { default: "EXAMPLCO", type: "string" },
 *       allowedAliases: { default: [] as string[], type: "string[]" },
 *     },
 *     run(file, ctx, opts) {
 *       for (const ln of file.lines()) {
 *         if (ln.isComment) continue;
 *         if (!ln.isStringLiteral) continue;
 *         const m = /\bExamplco\b/.exec(ln.text);
 *         if (!m) continue;
 *         if (opts.allowedAliases.some((a) => ln.text.includes(a))) continue;
 *         ctx.report({
 *           message: `Brand name must be "${opts.brand}", not "${m[0]}".`,
 *           span: ln.spanFor(m.index, m.index + m[0].length),
 *         });
 *       }
 *     },
 *   });
 */
export function defineCheck<const S extends OptionsSchema = {}>(
  input: DefineCheckInput<S>,
): Check<S> {
  // Spread first so any field on `input` beyond this SDK version's known
  // `DefineCheckInput` shape (e.g. a field added by a newer `.d.ts` than
  // what's actually installed, CD-94) still reaches the returned check
  // descriptor instead of being silently dropped by a hardcoded copy
  // list. Only the fields with real defaults get overridden below.
  return {
    ...input,
    defaultSeverity: input.defaultSeverity ?? "medium",
    requiresTypes: input.requiresTypes ?? false,
    outputMode: input.outputMode ?? false,
    consistency: input.consistency ?? false,
    options: input.options ?? ({} as S),
  } as Check<S>;
}
