// NoHttpClient — port of examplco-host's `NoBannedHttpClient` Credo
// check (cd-b5h e2e fixture). Pattern B / AST findAll.
//
// Flags imports of banned HTTP packages (axios, node-fetch, got, undici)
// outside an allowlisted internal wrapper module, and direct usages of
// the same modules' top-level identifiers in CallExpression position
// (e.g. `axios.get(...)` or `axios(...)`).
//
// Design intent:
//   - One `findAll("ImportDeclaration")` pass collects banned imports +
//     records the local names bound to each banned source so we can
//     match call sites that go through a renamed alias.
//   - One `findAll("CallExpression")` pass flags calls whose callee
//     resolves to a banned local name — either bare (`fetch(...)`) or
//     via a member chain (`axios.get(...)`).
//   - The plugin doesn't itself read the file's path against an
//     allowlist; that's the user's job via `cofferdam.toml`'s file
//     scoping (cd-81a.5). Plugin authors who only want this rule
//     enforced inside `lib/api/**` configure it there.

import {
  Category,
  defineCheck,
  Severity,
  type AstNode,
  type IdentifierReferenceNode,
  type MemberExpressionNode,
} from "@cofferdam/check-sdk";

const DEFAULT_BANNED = ["axios", "node-fetch", "got", "undici"];

export default defineCheck({
  id: "NoHttpClient",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.High,
  explanation:
    "Direct HTTP-client imports bypass the centralised wrapper " +
    "(retries, auth, observability). Route through the project's " +
    "internal HTTP module instead.",
  options: {
    bannedSources: { default: DEFAULT_BANNED, type: "string[]" },
    /** Module specifier prefixes whose imports are exempt — typically
     *  the internal wrapper that owns the allowed centralised path. */
    allowedSourcePrefixes: { default: [] as string[], type: "string[]" },
  },
  files: {
    extensions: ["ts", "tsx", "mts", "cts"],
  },
  run(file, ctx, opts) {
    if (!file.ast) return;

    const banned = new Set(opts.bannedSources);
    const allowed = opts.allowedSourcePrefixes;
    /** Map from local-binding name (e.g. `axios`, `myFetch`) to its
     *  banned source module (e.g. `"axios"`). Built from the import
     *  pass; consumed by the call-site pass. */
    const bannedLocals = new Map<string, string>();

    for (const imp of file.ast.findAll("ImportDeclaration")) {
      if (!banned.has(imp.source)) continue;
      if (allowed.some((prefix) => imp.source.startsWith(prefix))) continue;

      ctx.report({
        message:
          `Import of "${imp.source}" is banned — route HTTP through the internal wrapper.`,
        span: imp.span,
      });

      for (const spec of imp.specifiers) {
        bannedLocals.set(spec.localName, imp.source);
      }
    }

    if (bannedLocals.size === 0) return;

    for (const call of file.ast.findAll("CallExpression")) {
      const flagged = calleeResolvesToBanned(call.callee, bannedLocals);
      if (!flagged) continue;

      ctx.report({
        message:
          `Call site uses banned HTTP client "${flagged}" — route through the internal wrapper.`,
        span: call.span,
      });
    }
  },
});

/**
 * Walk a `CallExpression.callee` to determine whether its anchor
 * identifier is bound to a banned source. Returns the source string
 * when banned, `null` otherwise.
 *
 * Two shapes the v0 AstNode union cleanly handles:
 *   - `axios(...)` — `IdentifierReference` callee.
 *   - `axios.get(...)` / `axios.create().get(...)` — `MemberExpression`
 *     chain whose deepest `object` resolves to an `IdentifierReference`.
 */
function calleeResolvesToBanned(
  callee: AstNode,
  bannedLocals: Map<string, string>,
): string | null {
  let cursor: AstNode = callee;
  while (cursor.kind === "MemberExpression") {
    const next: AstNode = (cursor as MemberExpressionNode).object;
    cursor = next;
  }
  if (cursor.kind === "IdentifierReference") {
    return bannedLocals.get((cursor as IdentifierReferenceNode).name) ?? null;
  }
  return null;
}
