// Readability.MaxLineLength fixture (cd-c8aq): width is measured in display
// columns, not UTF-8 bytes. The multibyte lines below are all well under the
// 120-column limit and must NOT flag (byte-counting flagged them at ~3x).

// Decorative box-drawing banner — 48 display columns, 138 UTF-8 bytes:
// ─────────────────────────────────────────────
// Accented prose: Café costs €5 — résumé reviewed, naïve façade. Under limit.
const tags = ["一", "二", "三", "四", "五"]; // CJK literal, short line.

// The next line is a single long string literal — genuinely too wide, but
// unwrappable (CD-318): no formatter can break a literal in two, so flagging
// it would only pressure someone to delete content. Must NOT flag.
const aVeryLongConfigurationValue = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

// The next line is genuinely too wide AND made of many short, wrappable
// tokens — a formatter could reflow it across lines. SHOULD flag.
const total = computeSubtotal(itemOne, itemTwo) + computeTax(itemOne, itemTwo) + computeShipping(itemOne, itemTwo) + computeHandling(itemOne);
