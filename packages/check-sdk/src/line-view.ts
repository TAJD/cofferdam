// LineView — plugin-facing view of a source line with classification
// flags populated by the engine from the parsed comment list and an AST
// walk over string/template literals. Mirrors
// cofferdam_core::lines::LineView.
//
// Flag semantics (kept in sync with cofferdam-core/src/lines.rs):
//
// - `isComment` — any comment (line `//`, single-line block, or
//   multi-line block) overlaps this line.
// - `isDocComment` — a JSDoc-style block (`/** ... */`) overlaps. Implies
//   `isComment`.
// - `isStringLiteral` — a `StringLiteral` or `TemplateLiteral` span
//   overlaps this line *anywhere*. This is a whole-line flag, not a
//   span-level one: `<img src="/x.png" />` flags the entire line,
//   tag included, because a literal touches it somewhere (CD-100).
//   Use `stringLiteralRanges` to know *where* on the line a literal
//   sits instead of just whether one exists.
// - `isJsxText` — JSX text content overlaps this line. Filed as cd-0ne;
//   present in the SDK so plugins can target it as soon as the engine
//   ships the flag.
// - `isPragma` — an annotation-style comment (`/* #__PURE__ */`,
//   `/* @vite-ignore */`, etc.) overlaps. *Not* JSDoc.
// - `isTag` — HTML-only (CD-84): a tag (`<p>`, `</p>`, `<img ...>`, or a
//   doctype declaration) overlaps this line. Always `false` for
//   TS/JS/JSX files.
// - `isText` — HTML-only (CD-84): HTML text content overlaps this line.
//   Always `false` for TS/JS/JSX files. Not the same concept as
//   `isJsxText` — `isJsxText` marks JSX child text inside a `.tsx` file.

import type { Span } from "./span.js";

export interface LineView {
  /** 1-based line number. */
  readonly lineNo: number;
  /** Line text with the trailing `\r` (CRLF) stripped, no `\n`. */
  readonly text: string;
  readonly isComment: boolean;
  readonly isDocComment: boolean;
  readonly isStringLiteral: boolean;
  /**
   * Byte ranges `[start, end)` *within this line* (relative to `text`,
   * after CRLF stripping) covered by a string/template literal, clipped
   * to the line's own bounds. The span-aware counterpart to
   * `isStringLiteral` (CD-100) — lets a check skip only the literal
   * text on a line instead of the whole line. Empty when the line has
   * no literal, or for languages with no literal-span data (HTML,
   * Astro).
   */
  readonly stringLiteralRanges: ReadonlyArray<readonly [number, number]>;
  readonly isJsxText: boolean;
  readonly isPragma: boolean;
  readonly isTag: boolean;
  readonly isText: boolean;

  /**
   * Build a `Span` covering `[charStart, charEnd)` *within this line*.
   * `charStart`/`charEnd` are UTF-16 code-unit indices into `text` — the
   * same units `String.prototype.indexOf`/`.search()` and a regex
   * match's `.index` use, so `ln.spanFor(m.index, m.index + m[0].length)`
   * for a match against `line.text` is always correct. The returned
   * span's `start_byte`/`end_byte`/`column` are converted to UTF-8 bytes
   * internally (CD-191) and are file-absolute, for direct use in
   * `ctx.report({ span })`.
   *
   * Filed as cd-cgd to keep authoring concise — without this the check
   * has to reconstruct the file-absolute offset by hand from the line's
   * own start.
   */
  spanFor(charStart: number, charEnd: number): Span;
}
