// defineCheck — the factory plugin authors call to declare a check.
//
// Type inference flows from the `options` schema declared on the input
// to the `opts` argument of `run`. This is the surface that earns the
// SDK its right to exist — without it, plugin authors hand-write
// matching types twice.

import type { Category } from "./category.js";
import type { CheckContext, SourceFile } from "./check-context.js";
import type { OptionsSchema, ResolvedOptions } from "./options.js";
import type { Severity } from "./severity.js";

/**
 * Declarative file-scope filter (cd-81a.5). When set, the engine
 * pre-filters discovered files against this scope before invoking
 * `run()`. Replaces the `scannable?` boilerplate that rovikore-host
 * checks ship today.
 */
export interface FileScope {
  /** File extensions (without leading dot). Empty/omitted = any. */
  readonly extensions?: readonly string[];
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
   * Whether the check needs full type-aware analysis (routes through ts-morph).
   *
   * **Status (0.2.x): not yet implemented.** Setting this to `true` is
   * accepted by the type system and recorded on the `Check` object, but the
   * engine does **not** route the check through ts-morph and no type
   * information is made available inside `run()`. The field is reserved for
   * future use; type-aware routing via ts-morph is on the roadmap (tracked
   * as cd-l58 / gh #16). Until that work lands, treat `requiresTypes: true`
   * as a declaration of intent only — the check will still run, but without
   * type data.
   */
  readonly requiresTypes: boolean;
  /** Two-pass consistency mode. */
  readonly consistency: boolean;
  readonly options: S;
  /** File-scope filter. `undefined` = applies to every file. */
  readonly files: FileScope | undefined;
  run(file: SourceFile, ctx: CheckContext, opts: ResolvedOptions<S>): void;
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
   * Whether the check needs full type-aware analysis (routes through ts-morph).
   *
   * **Status (0.2.x): not yet implemented.** Setting this to `true` is
   * accepted by the type system and recorded on the `Check` object, but the
   * engine does **not** route the check through ts-morph and no type
   * information is made available inside `run()`. The field is reserved for
   * future use; type-aware routing via ts-morph is on the roadmap (tracked
   * as cd-l58 / gh #16). Until that work lands, treat `requiresTypes: true`
   * as a declaration of intent only — the check will still run, but without
   * type data. The plugin host will emit a one-time warning to stderr at
   * load time when this is `true`.
   */
  readonly requiresTypes?: boolean;
  readonly consistency?: boolean;
  /** Schema; defaults to `{}`. Inferred at the call site. */
  readonly options?: S;
  /** File-scope filter; defaults to "every file". */
  readonly files?: FileScope;
  run(file: SourceFile, ctx: CheckContext, opts: ResolvedOptions<S>): void;
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
 *       brand: { default: "ROVIKORE", type: "string" },
 *       allowedAliases: { default: [] as string[], type: "string[]" },
 *     },
 *     run(file, ctx, opts) {
 *       for (const ln of file.lines()) {
 *         if (ln.isComment) continue;
 *         if (!ln.isStringLiteral) continue;
 *         const m = /\bRovikore\b/.exec(ln.text);
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
  return {
    id: input.id,
    category: input.category,
    basePriority: input.basePriority,
    defaultSeverity: input.defaultSeverity ?? "medium",
    explanation: input.explanation,
    body: input.body,
    requiresTypes: input.requiresTypes ?? false,
    consistency: input.consistency ?? false,
    options: input.options ?? ({} as S),
    files: input.files,
    run: input.run,
  };
}
