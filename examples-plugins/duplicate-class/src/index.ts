// DuplicateClassName — example cross-file plugin (cd-9hp.6).
//
// Aggregates every `class Foo { ... }` declaration across the corpus
// via the plugin corpus (`ctx.corpus`), then in `finalize` emits one
// finding per class name declared in more than one file.
//
// Demonstrates the v1 cross-file plugin pattern:
//   1. Per-file `run` appends a record to a corpus slot.
//   2. `finalize` reads the slot back, groups, and emits findings whose
//      primary location is the first declaration and whose `related`
//      list carries every duplicate.
//
// The pattern is intentionally minimal — no AST type-resolution, no
// project-graph queries. Plugins targeting richer cross-file rules
// today still have to do the heavy lifting themselves; cd-9hp.9
// (canonical graph) is the substrate that lets the SDK ship those
// queries built-in.

import {
  defineCheck,
  Category,
  Severity,
  type Span,
} from "@cofferdam/check-sdk";

interface ClassDecl {
  readonly file: string;
  readonly name: string;
  readonly span: Span;
}

const SLOT = "classes";

export default defineCheck({
  id: "DuplicateClassName",
  category: Category.Design,
  basePriority: 8,
  defaultSeverity: Severity.Medium,
  explanation:
    "A `class` name is declared in more than one file. Pick one canonical declaration and rename or remove the rest.",
  options: {},
  files: { extensions: ["ts", "tsx"] },
  run(file, ctx) {
    if (!file.ast) return;
    for (const cls of file.ast.findAll("Class")) {
      if (!cls.name) continue;
      ctx.corpus.append<ClassDecl>(SLOT, {
        file: file.path,
        name: cls.name,
        span: cls.span,
      });
    }
  },
  finalize(ctx) {
    const all = ctx.corpus.read<ClassDecl[]>(SLOT) ?? [];
    if (all.length === 0) return;

    const byName = new Map<string, ClassDecl[]>();
    for (const decl of all) {
      const list = byName.get(decl.name);
      if (list) {
        list.push(decl);
      } else {
        byName.set(decl.name, [decl]);
      }
    }

    for (const [name, decls] of byName) {
      // Only flag *cross-file* duplicates. Same-file duplicates are a
      // different rule (TypeScript itself errors on them) and would
      // surface here as noise.
      const uniqueFiles = new Set(decls.map((d) => d.file));
      if (uniqueFiles.size < 2) continue;

      // Deterministic primary: lexicographically smallest file path,
      // then earliest byte offset. Keeps the finding stable across runs.
      const sorted = [...decls].sort((a, b) =>
        a.file === b.file
          ? a.span.start_byte - b.span.start_byte
          : a.file < b.file
            ? -1
            : 1,
      );
      const primary = sorted[0]!;
      const others = sorted.slice(1);

      ctx.report({
        file: primary.file,
        message: `class "${name}" is declared in ${uniqueFiles.size} files; pick a single canonical declaration.`,
        span: primary.span,
        related: others.map((d) => ({ file: d.file, span: d.span })),
      });
    }
  },
});
