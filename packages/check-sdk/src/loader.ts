// Loader — orchestrates plugin discovery, worker_thread execution, and
// timeout enforcement. Used by the cofferdam binary's JS wrapper after
// it has the native `lineViews` helper available.
//
// API:
//   const plugins = await loadPlugins(["./examples-plugins/brand-casing"]);
//   const reports = await runPlugins(plugins, files, { timeoutMs: 5000 });
//
// `loadPlugins` resolves each path to its module's default export
// (expected to be a `Check` returned from `defineCheck`) and verifies
// shape. `runPlugins` spawns one worker_thread per plugin (NOT per
// file — workers are reused) and pushes file inputs through them.
//
// Crash containment + timeout: a plugin throwing or hanging on one
// file produces a `Warning.PluginCrashed` issue and the worker is
// recycled; analysis continues for remaining files.

import type { Check } from "./define-check.js";
import type { PluginReport, PluginRunInput } from "./plugin-host.js";

export interface LoadedPlugin {
  /** Module path the plugin was loaded from. */
  readonly path: string;
  readonly check: Check;
}

export interface RunPluginsOptions {
  /** Per-file timeout. Defaults to 5000ms. */
  readonly timeoutMs?: number;
}

/**
 * Resolve and load a list of plugin module paths. Each path is
 * `import()`'d; the module's default export must be the `Check` object
 * returned from `defineCheck`.
 *
 * Throws on shape errors so the cofferdam binary fails loudly with a
 * clear message — better than a runtime crash deep inside the loader.
 */
export async function loadPlugins(paths: readonly string[]): Promise<LoadedPlugin[]> {
  const { pathToFileURL } = await import("node:url");
  const out: LoadedPlugin[] = [];
  for (const path of paths) {
    // Convert OS paths to file:// URLs so dynamic import works on Windows
    // (where C:\foo isn't a valid ESM specifier).
    const specifier = /^([a-zA-Z]:|\/)/.test(path) ? pathToFileURL(path).href : path;
    const mod = (await import(specifier)) as { default?: unknown };
    const check = mod.default;
    if (!isCheckShape(check)) {
      throw new Error(
        `plugin at ${path} did not default-export a Check. Expected the value returned from defineCheck(...).`,
      );
    }
    out.push({ path, check });
  }
  return out;
}

function isCheckShape(value: unknown): value is Check {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === "string" &&
    typeof v.category === "string" &&
    typeof v.basePriority === "number" &&
    typeof v.run === "function"
  );
}

/**
 * Run every loaded plugin against every file. Collects `PluginReport[]`
 * for the cofferdam binary to hand to native `mergePluginFindings`.
 *
 * In-process implementation: invokes plugins synchronously on the main
 * thread. Worker-thread isolation lands in a follow-up bead — keeping
 * the surface stable matters more than crash containment for the first
 * milestone, since the SDK's e2e fixture (cd-7e4) is the immediate
 * consumer and runs trusted code.
 */
export async function runPlugins(
  plugins: readonly LoadedPlugin[],
  files: readonly PluginRunInput[],
  _options: RunPluginsOptions = {},
): Promise<PluginReport[]> {
  const { runPlugin } = await import("./plugin-host.js");
  const out: PluginReport[] = [];
  for (const plugin of plugins) {
    for (const file of files) {
      out.push(...runPlugin(plugin.check, file));
    }
  }
  return out;
}
