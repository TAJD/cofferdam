// Value export. The canonical-graph build pass records this file's
// incoming consumer (consumer.ts) as an `ImportsAsType` edge because
// the consumer's import statement is `import type`. OrphanExport
// MUST count that edge as consumption — otherwise tightening
// type-only imports would mass-orphan value exports that nothing
// imports in value position.
export const Widget = { kind: "box" };
