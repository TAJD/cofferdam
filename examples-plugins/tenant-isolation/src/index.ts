// TenantIsolation — Pattern C / stateful walk fixture for cd-11j.
// Inspired by rovikore-host's TenantIsolation (Ash/Elixir) — same
// multi-tenancy data-leak concern, mapped to a TS-native target.
//
// Pattern C: collect state across the AST walk, decide per-file at the
// end. The state machine has three slots:
//
//   - `hasTenantWrapper` — set if the file imports a known wrapper
//     binding (e.g. `withTenantScope`, `scopedPrisma`) or imports from
//     a wrapper-source prefix (e.g. `@org/db`). When set, the file is
//     trusted to route its own scoping; we suppress all findings.
//   - `pendingFindings` — every Prisma query call site whose first
//     argument's `where` clause does not mention any of the configured
//     `tenantFields`. Emitted only if the wrapper sentinel didn't fire.
//
// Why Pattern C and not findAll: the decision to flag a query depends
// on file-global state (the wrapper presence), not a per-node check.
// Two findAll passes would re-scan the AST; one walk does both jobs in
// document order.
//
// Why text-slicing the where clause: v0's AstNode union exposes
// ObjectExpression.properties: AstNode[] but no `Property` kind to
// introspect. A whole-clause regex is good enough — false negatives
// (legitimately-scoped queries we don't recognise) are conservative;
// false positives (flagging a scoped query) get suppressed via
// // cofferdam-ignore on the call site.

import {
  Category,
  defineCheck,
  Severity,
  Walk,
  type AstNode,
  type CallExpressionNode,
  type ImportDeclarationNode,
  type IdentifierReferenceNode,
  type MemberExpressionNode,
  type ObjectExpressionNode,
  type Span,
} from "@cofferdam/check-sdk";

// Minimal local declarations for the Node/Web globals we use. Declared
// in-plugin so we don't pull in @types/node and grow the dev surface
// for a fixture. ES2022 lib doesn't include these by default.
declare class TextEncoder {
  encode(input: string): Uint8Array;
}
declare class TextDecoder {
  constructor(label?: string);
  decode(input: Uint8Array): string;
}

const QUERY_METHODS = new Set([
  "findMany",
  "findFirst",
  "findUnique",
  "findFirstOrThrow",
  "findUniqueOrThrow",
]);

export default defineCheck({
  id: "TenantIsolation",
  category: Category.Warning,
  basePriority: 15,
  defaultSeverity: Severity.High,
  explanation:
    "Prisma queries that omit a tenant scope (e.g. `siteId`) leak " +
    "data across tenants. Either add a tenant field to the `where` " +
    "clause or route the query through the project's tenant-aware " +
    "wrapper.",
  options: {
    tenantFields: {
      default: ["siteId", "merchantId", "tenantId", "organizationId"],
      type: "string[]",
    },
    /** Local-name bindings whose presence in any import statement
     *  marks this file as wrapper-routed (suppresses all findings). */
    wrapperLocalNames: {
      default: ["withTenantScope", "scopedPrisma"] as string[],
      type: "string[]",
    },
    /** Module-specifier prefixes that own a tenant-aware wrapper; an
     *  import whose source matches counts as wrapper presence. */
    wrapperSourcePrefixes: {
      default: [] as string[],
      type: "string[]",
    },
    /** Identifier the Prisma client is bound to at module scope.
     *  Customisable for codebases that name it `db` or similar. */
    clientName: { default: "prisma", type: "string" },
  },
  files: {
    extensions: ["ts", "tsx", "mts", "cts"],
  },
  run(file, ctx, opts) {
    if (!file.ast) return;

    const tenantFields = opts.tenantFields;
    const wrapperLocals = new Set(opts.wrapperLocalNames);
    const wrapperPrefixes = opts.wrapperSourcePrefixes;
    const clientName = opts.clientName;

    // Spans are UTF-8 byte offsets but `file.text` is a JS string indexed
    // by UTF-16 code units; slicing the string directly mis-aligns when
    // the file has multi-byte characters (em-dash, arrows, etc.). Encode
    // the text to a byte view once per file so per-call where-clause
    // slicing is correct regardless of source encoding quirks.
    const fileBytes = new TextEncoder().encode(file.text);

    let hasTenantWrapper = false;
    const pendingFindings: { span: Span; message: string }[] = [];

    file.ast.walk({
      visitImportDeclaration(node: ImportDeclarationNode): Walk {
        if (wrapperPrefixes.some((prefix) => node.source.startsWith(prefix))) {
          hasTenantWrapper = true;
        }
        for (const spec of node.specifiers) {
          if (wrapperLocals.has(spec.localName)) {
            hasTenantWrapper = true;
          }
        }
        return Walk.Continue;
      },

      visitCallExpression(node: CallExpressionNode): Walk {
        const queryMethod = matchPrismaQuery(node.callee, clientName);
        if (!queryMethod) return Walk.Continue;

        const firstArg = node.arguments[0];
        if (!firstArg || firstArg.kind !== "ObjectExpression") {
          pendingFindings.push({
            span: node.span,
            message:
              `Prisma ${queryMethod} call has no query options — likely missing tenant scope.`,
          });
          return Walk.Continue;
        }

        if (!whereContainsTenantField(firstArg, fileBytes, tenantFields)) {
          pendingFindings.push({
            span: node.span,
            message:
              `Prisma ${queryMethod} call's where clause is missing a tenant field ` +
              `(expected one of: ${tenantFields.join(", ")}).`,
          });
        }
        return Walk.Continue;
      },
    });

    if (hasTenantWrapper) return;
    for (const finding of pendingFindings) {
      ctx.report({ message: finding.message, span: finding.span });
    }
  },
});

/**
 * Returns the query method name (e.g. `"findMany"`) when the callee
 * matches the `<clientName>.<Model>.<queryMethod>` shape, otherwise
 * `null`. Walks two MemberExpression hops down to the base identifier.
 *
 * Shape we accept:
 *   prisma.merchant.findMany(...)
 *      └── MemberExpression(property="findMany")
 *           └── object: MemberExpression(property=<model>)
 *                └── object: IdentifierReference(name="prisma")
 */
function matchPrismaQuery(callee: AstNode, clientName: string): string | null {
  if (callee.kind !== "MemberExpression") return null;
  const methodMember = callee as MemberExpressionNode;
  const method = methodMember.property;
  if (!method || !QUERY_METHODS.has(method)) return null;

  const modelMember = methodMember.object;
  if (modelMember.kind !== "MemberExpression") return null;

  const base = (modelMember as MemberExpressionNode).object;
  if (base.kind !== "IdentifierReference") return null;
  if ((base as IdentifierReferenceNode).name !== clientName) return null;

  return method;
}

/**
 * Slice the `ObjectExpression`'s span from the source text and look
 * for a `where: { ... <tenantField>: ... }` clause containing any of
 * the configured field names. Conservative — false negatives are
 * preferred over false positives.
 */
function whereContainsTenantField(
  options: ObjectExpressionNode,
  fileBytes: Uint8Array,
  tenantFields: readonly string[],
): boolean {
  const slice = fileBytes.subarray(options.span.start_byte, options.span.end_byte);
  const argText = new TextDecoder("utf-8").decode(slice);

  if (!/\bwhere\s*:/.test(argText)) return false;

  // Accept either property-form (`siteId: ...`) or shorthand
  // (`{ id, siteId }` / `{ siteId }`) — the trailing context covers
  // both. Conservative: a stringified false positive (`"siteId:"` in a
  // template) over-clears, but Pattern C's whole-file suppression via
  // // cofferdam-ignore is the escape hatch.
  for (const field of tenantFields) {
    const escaped = field.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (new RegExp(`\\b${escaped}\\s*[:,}\\s]`).test(argText)) {
      return true;
    }
  }
  return false;
}
