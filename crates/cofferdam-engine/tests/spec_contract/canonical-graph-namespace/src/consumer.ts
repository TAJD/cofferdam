// Namespace import — lays down a single canonical-graph edge with
// `import_kind == "namespace"`. OrphanExport reads that as
// "every named export on `./lib` is consumed"; the default export
// is NOT shielded because namespace imports don't reach defaults.
//
// No exports here — keeps the fixture's expected.json focused on
// the namespace-edge consumption rule.
import * as Lib from "./lib";

declare const _x: number;
void (_x === Lib.alpha + Lib.beta);
