// `import type { Widget }` lands in the canonical graph as an
// `EdgeKind::ImportsAsType` edge into widgets.ts. The consumer
// uses the imported binding strictly in a type position
// (`typeof Widget`), making the type-only form legitimate.
//
// Deliberately no exports here so the fixture's expected.json
// captures only the type-only consumption assertion — there is
// nothing on this file for OrphanExport to flag.
import type { Widget } from "./widgets";

declare const _x: typeof Widget;
void _x;
