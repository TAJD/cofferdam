// Bare side-effect import — no names, no default. The canonical
// graph records ONE edge with `import_kind == "side_effect"`.
// OrphanExport skips side_effect edges in its consumption summary,
// so polyfill.ts's named export stays orphan even though this file
// references it.
//
// Deliberately no exports here — keeps the fixture's expected.json
// focused on the side-effect-edge consumption rule.
import "./polyfill";
