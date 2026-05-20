// Per-check options schema (cd-81a.3). Plugin authors declare an
// `options` block on their `defineCheck(...)` call; the engine validates
// user `cofferdam.toml` overrides against it at startup, and the resolved
// values are passed to `run(file, ctx, opts)` typed correctly.
//
// The schema is a record of name → { default, type }. The `type` field is
// the discriminator that drives both runtime validation and the type
// inference below.

export type OptionKind = "string" | "number" | "boolean" | "string[]" | "number[]";

export interface OptionSpec<K extends OptionKind, D> {
  readonly type: K;
  readonly default: D;
  /** Optional human-readable description, surfaced by `cofferdam explain`. */
  readonly description?: string;
}

// ---- Type-level mapping from OptionKind to the resolved value type ----

type ResolvedFromKind<K extends OptionKind> = K extends "string"
  ? string
  : K extends "number"
    ? number
    : K extends "boolean"
      ? boolean
      : K extends "string[]"
        ? readonly string[]
        : K extends "number[]"
          ? readonly number[]
          : never;

/**
 * The shape of `opts` inside a check's `run(file, ctx, opts)` callback —
 * derived from the schema declared on `defineCheck.options`.
 *
 * @example
 *   const sdk = defineCheck({
 *     options: {
 *       brand: { default: "EXAMPLCO", type: "string" },
 *       allowedAliases: { default: [] as string[], type: "string[]" },
 *     },
 *     run(file, ctx, opts) {
 *       opts.brand;          // string
 *       opts.allowedAliases; // readonly string[]
 *     },
 *   });
 */
export type ResolvedOptions<S> = {
  readonly [K in keyof S]: S[K] extends OptionSpec<infer Kind, infer _D>
    ? ResolvedFromKind<Kind>
    : never;
};

/** A bare schema record — `Record<string, OptionSpec<...>>`. Constrained
 *  generics on `defineCheck` use this as the `extends` upper bound. */
export type OptionsSchema = Record<string, OptionSpec<OptionKind, unknown>>;

/** Empty schema — used as the default in `defineCheck` so the
 *  third `opts` argument is `Record<string, never>` for option-less
 *  checks rather than `undefined`. */
export const NO_OPTIONS = {} as const;
export type NoOptions = typeof NO_OPTIONS;
