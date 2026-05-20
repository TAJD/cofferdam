// AST surface for plugins (cd-81a.2). Mirrors cofferdam_core::ast — a
// thin TS proxy over oxc's parsed tree. The napi loader serializes the
// shape across the FFI boundary so plugins program against this.
//
// Three access patterns:
//
// 1. Eager filter — `view.findAll("CallExpression")` returns every node
//    of the given kind in document order.
// 2. Stateful walk — `view.walk(visitor)`. Each visit method returns
//    `Walk.Continue` or `Walk.Skip` to control descent.
// 3. Direct rooted access — `view.root` for surgical inspection. Escape
//    hatch when the kinds we expose don't cover the case yet.

import type { Span } from "./span.js";

/** Outcome of an AstVisitor visit. Controls per-node descent. */
export const Walk = {
  Continue: "continue",
  Skip: "skip",
} as const;
export type Walk = (typeof Walk)[keyof typeof Walk];

/**
 * Kinds exposed via {@link AstView.findAll} and the {@link AstNode}
 * union. Deliberately a small set covering the examplco-host plugin
 * patterns; grows as plugin needs surface.
 *
 * v0 freeze: cd-717 (`design/sdk-ast-surface.md`). 5 strict +
 * 4 borderline kinds. `NewExpression`, `BinaryExpression`,
 * `StringLiteral`, `NumericLiteral` are deferred until a fixture
 * justifies them — adding kinds is non-breaking, removing isn't.
 */
export type NodeKind =
  | "Program"
  | "CallExpression"
  | "ImportDeclaration"
  | "Function"
  | "ArrowFunctionExpression"
  | "Class"
  | "ObjectExpression"
  | "MemberExpression"
  | "IdentifierReference";

/**
 * Common base for every AST node exposed across the FFI. Concrete
 * kinds extend this with their own typed properties.
 */
export interface AstNodeBase<K extends NodeKind> {
  readonly kind: K;
  readonly span: Span;
}

// ---- typed kind shapes (subset; expand as needed by plugins) ---------

/**
 * The Program root — entry point for `view.root`. Body holds top-level
 * statements; iterate it for declarations / imports / exports without
 * walking the rest of the tree.
 *
 * Added in cd-svf to make `view.root: AstNode` typeable. Without
 * Program in the union, root narrows to `never` on any TS switch.
 */
export interface ProgramNode extends AstNodeBase<"Program"> {
  readonly body: readonly AstNode[];
}

export interface CallExpressionNode extends AstNodeBase<"CallExpression"> {
  /** The callee expression — typically an `IdentifierReference` or `MemberExpression`. */
  readonly callee: AstNode;
  readonly arguments: readonly AstNode[];
}

export interface ImportDeclarationNode extends AstNodeBase<"ImportDeclaration"> {
  /** The module specifier, e.g. `"axios"`. */
  readonly source: string;
  /** Each specifier's local name. */
  readonly specifiers: readonly { readonly localName: string; readonly imported?: string }[];
}

export interface FunctionNode extends AstNodeBase<"Function"> {
  readonly name: string | undefined;
  readonly params: readonly AstNode[];
  readonly async: boolean;
  readonly generator: boolean;
}

export interface ArrowFunctionExpressionNode extends AstNodeBase<"ArrowFunctionExpression"> {
  readonly params: readonly AstNode[];
  readonly async: boolean;
  readonly expression: boolean;
}

export interface ClassNode extends AstNodeBase<"Class"> {
  readonly name: string | undefined;
}

export interface ObjectExpressionNode extends AstNodeBase<"ObjectExpression"> {
  readonly properties: readonly AstNode[];
}

export interface MemberExpressionNode extends AstNodeBase<"MemberExpression"> {
  readonly object: AstNode;
  /** For `.foo` / `["foo"]`, the resolved property name when statically determinable. */
  readonly property: string | undefined;
  readonly computed: boolean;
}

export interface IdentifierReferenceNode extends AstNodeBase<"IdentifierReference"> {
  readonly name: string;
}

/** Discriminated union of every node kind {@link AstView} surfaces. */
export type AstNode =
  | ProgramNode
  | CallExpressionNode
  | ImportDeclarationNode
  | FunctionNode
  | ArrowFunctionExpressionNode
  | ClassNode
  | ObjectExpressionNode
  | MemberExpressionNode
  | IdentifierReferenceNode;

/** Map from kind string to typed node — drives `findAll`'s return type. */
export type NodeOfKind<K extends NodeKind> = Extract<AstNode, { kind: K }>;

/**
 * Visitor passed to {@link AstView.walk}. Override only the kinds you
 * care about; defaults are `Walk.Continue` no-ops. Returning
 * `Walk.Skip` stops descent into that node only — sibling nodes still
 * visit.
 */
export interface AstVisitor {
  visitProgram?(node: ProgramNode): Walk;
  visitCallExpression?(node: CallExpressionNode): Walk;
  visitImportDeclaration?(node: ImportDeclarationNode): Walk;
  visitFunction?(node: FunctionNode): Walk;
  visitArrowFunctionExpression?(node: ArrowFunctionExpressionNode): Walk;
  visitClass?(node: ClassNode): Walk;
  visitObjectExpression?(node: ObjectExpressionNode): Walk;
  visitMemberExpression?(node: MemberExpressionNode): Walk;
  visitIdentifierReference?(node: IdentifierReferenceNode): Walk;
}

/**
 * Borrowed view of a parsed file's AST. Cheap to construct; cofferdam's
 * napi loader builds one per file per check invocation.
 */
export interface AstView {
  /** Direct access to the program root for surgical inspection. */
  readonly root: AstNode;

  /**
   * Eagerly collect every node of `kind` in document order. O(N) over
   * the AST. Allocates an array. For multi-kind queries prefer a single
   * {@link walk} pass.
   */
  findAll<K extends NodeKind>(kind: K): readonly NodeOfKind<K>[];

  /**
   * Run a stateful visitor over the tree. See {@link AstVisitor} for
   * the per-kind callbacks.
   */
  walk(visitor: AstVisitor): void;
}
