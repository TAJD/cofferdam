// @cofferdam/check-sdk — author cofferdam plugin checks in TypeScript.
//
// Plugin authors install this package, write a check with `defineCheck`,
// and `default export` it. The cofferdam napi loader (cd-81a.7) picks
// the module up via `cofferdam.toml`'s `plugins = [...]` array and runs
// it in a worker_thread per source file.
//
// This is the public surface. Everything exported here is part of the
// stability contract; everything kept internal is not. New exports are
// additive — removing or renaming an export is a breaking change.

export { Category } from "./category.js";
export { Severity } from "./severity.js";
export { Walk } from "./ast.js";
export { defineCheck } from "./define-check.js";

// Loader runtime (cd-81a.7). Plugin authors import only the surfaces
// above; cofferdam's own JS wrapper is the consumer of these.
export { runPlugin, buildSourceFile } from "./plugin-host.js";
export { loadPlugins, runPlugins } from "./loader.js";
export type {
  PluginReport,
  PluginRunInput,
  NativeLineView,
} from "./plugin-host.js";
export type { LoadedPlugin, RunPluginsOptions } from "./loader.js";

export type { Check, DefineCheckInput, FileScope } from "./define-check.js";
export type { CheckContext, Fix, ReportArgs, SourceFile } from "./check-context.js";
export type { LineView } from "./line-view.js";
export type { Span, RelatedSpan } from "./span.js";
export type {
  AstView,
  AstVisitor,
  AstNode,
  AstNodeBase,
  CallExpressionNode,
  ImportDeclarationNode,
  FunctionNode,
  ArrowFunctionExpressionNode,
  ClassNode,
  ObjectExpressionNode,
  MemberExpressionNode,
  IdentifierReferenceNode,
  NodeKind,
  NodeOfKind,
} from "./ast.js";
export type {
  OptionKind,
  OptionSpec,
  OptionsSchema,
  ResolvedOptions,
  NoOptions,
} from "./options.js";
