// Default-via-named-rename — `import { default as W }` lands in
// the canonical graph as a Named edge with `source_name ==
// "default"`. OrphanExport's per-file consumption summary reroutes
// that into the default bucket so widget.ts's default export is
// consumed (not orphan).
//
// No exports here — keeps the fixture's expected.json focused on
// the default-rerouting rule.
import { default as W } from "./widget";

declare const _x: string;
void (_x === W("hello"));
