// Fixture for Context.BlastRadius — depth-3 (transitive) consumer:
// imports wrapper.ts, which imports direct_caller.ts, which imports
// the changed lib.ts.
import { wrap } from "./wrapper";

export function run(): string {
  return wrap();
}
