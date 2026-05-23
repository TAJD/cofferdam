// Readability.MaxLineLength fixture (cd-c8aq): width is measured in display
// columns, not UTF-8 bytes. The multibyte lines below are all well under the
// 120-column limit and must NOT flag (byte-counting flagged them at ~3x).

// Decorative box-drawing banner — 48 display columns, 138 UTF-8 bytes:
// ─────────────────────────────────────────────
// Accented prose: Café costs €5 — résumé reviewed, naïve façade. Under limit.
const tags = ["一", "二", "三", "四", "五"]; // CJK literal, short line.

// The next line is genuinely too wide (well over 120 ASCII columns) and SHOULD flag:
const aVeryLongConfigurationValue = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
