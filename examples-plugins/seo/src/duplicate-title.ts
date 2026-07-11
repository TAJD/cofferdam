// SeoDuplicateTitle — HTML AstView (CD-83/84) + ctx.corpus/finalize
// cross-file pattern (mirrors DuplicateClassName in
// examples-plugins/duplicate-class/src/index.ts), extended to also
// catch duplicate canonical URLs.
//
// `outputMode: true` (CD-85) makes this check eligible for `cofferdam
// verify --dist` against a build's `.html` output, in addition to
// running normally under `cofferdam check` against any `.html` files in
// scope — source-level and output-level SEO checks are complementary,
// so one check covers both (CD-86).

import {
  defineCheck,
  Category,
  Severity,
  type Span,
} from "@cofferdam/check-sdk";

interface TitleRecord {
  readonly file: string;
  readonly text: string;
  readonly span: Span;
}

interface CanonicalRecord {
  readonly file: string;
  readonly href: string;
  readonly span: Span;
}

const TITLES_SLOT = "titles";
const CANONICALS_SLOT = "canonicals";

function reportDuplicates<T extends { file: string; span: Span }>(
  ctx: { report(args: { file: string; message: string; span: Span; related: readonly { file: string; span: Span }[] }): void },
  records: readonly T[],
  keyOf: (record: T) => string,
  messageFor: (key: string, uniqueFiles: number) => string,
): void {
  const byKey = new Map<string, T[]>();
  for (const rec of records) {
    const list = byKey.get(keyOf(rec));
    if (list) {
      list.push(rec);
    } else {
      byKey.set(keyOf(rec), [rec]);
    }
  }

  for (const [key, recs] of byKey) {
    const uniqueFiles = new Set(recs.map((r) => r.file));
    if (uniqueFiles.size < 2) continue;

    // Deterministic primary: lexicographically smallest file path, then
    // earliest byte offset. Keeps the finding stable across runs.
    const sorted = [...recs].sort((a, b) =>
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
      message: messageFor(key, uniqueFiles.size),
      span: primary.span,
      related: others.map((r) => ({ file: r.file, span: r.span })),
    });
  }
}

export default defineCheck({
  id: "SeoDuplicateTitle",
  category: Category.Warning,
  basePriority: 10,
  defaultSeverity: Severity.Medium,
  explanation:
    "A page's <title> text or canonical URL is duplicated across more than one page — search engines penalize duplicate titles/canonicals for SEO.",
  outputMode: true,
  files: { extensions: ["html"] },
  run(file, ctx) {
    if (!file.ast) return;
    for (const el of file.ast.findAll("Element")) {
      if (el.tagName === "title") {
        const text = el.children
          .filter((c) => c.kind === "Text")
          .map((c) => c.text)
          .join("")
          .trim();
        if (text) {
          ctx.corpus.append<TitleRecord>(TITLES_SLOT, { file: file.path, text, span: el.span });
        }
        continue;
      }
      if (el.tagName === "link") {
        const rel = el.attributes.find((a) => a.name === "rel");
        if (rel?.value !== "canonical") continue;
        const href = el.attributes.find((a) => a.name === "href");
        if (href?.value) {
          ctx.corpus.append<CanonicalRecord>(CANONICALS_SLOT, {
            file: file.path,
            href: href.value,
            span: el.span,
          });
        }
      }
    }
  },
  finalize(ctx) {
    const titles = ctx.corpus.read<TitleRecord[]>(TITLES_SLOT) ?? [];
    reportDuplicates(
      ctx,
      titles,
      (r) => r.text,
      (text, count) => `<title>"${text}" is duplicated across ${count} files.`,
    );

    const canonicals = ctx.corpus.read<CanonicalRecord[]>(CANONICALS_SLOT) ?? [];
    reportDuplicates(
      ctx,
      canonicals,
      (r) => r.href,
      (href, count) => `canonical URL "${href}" is duplicated across ${count} files.`,
    );
  },
});
