// Fixture for Context.BlastRadius — the exported symbol this fixture's
// callers depend on. A synthetic signature change here (see the CLAUDE
// task description / CD-160) should put direct_caller.ts and
// lib.test.ts in the digest top-3.
export function doThing(x: number): string {
  return String(x);
}
