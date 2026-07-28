// Fixture for Design.MissingTestFile (CD-146) — no errors.test.ts
// exists, but covered.test.ts imports this class and exercises it via
// `instanceof` / `toThrow`, so it must NOT be flagged.
export class ValidationError extends Error {}
