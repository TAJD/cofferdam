// Mirror of cofferdam_core::Span. Carries both human (line/column) and
// machine (byte offsets) coordinates so `ctx.report` consumers can do
// either without re-parsing.
//
// All coordinates are 1-based for line, 1-based for column, 0-based for
// bytes — matching the Rust struct and the JSON formatter contract.

export interface Span {
  /** 1-based line number. */
  readonly line: number;
  /** 1-based column number, in UTF-8 bytes from the start of the line. */
  readonly column: number;
  /**
   * 0-based UTF-8 *byte* offset from the start of the file (inclusive).
   * NOT a JS string index: `SourceFile.text` is UTF-16-indexed, so
   * `text.slice(start_byte, end_byte)` is only correct when every byte
   * before `start_byte` is ASCII (CD-191). Use `SourceFile.textAt(span)`
   * to get the covered text safely.
   */
  readonly start_byte: number;
  /** 0-based byte offset from the start of the file (exclusive). Same caveat as {@link start_byte}. */
  readonly end_byte: number;
}

export interface RelatedSpan {
  readonly file: string;
  readonly span: Span;
}
