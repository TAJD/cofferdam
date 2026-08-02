// Fixture for Context.BlastRadius — depth-2 (transitive) consumer:
// imports direct_caller.ts, which imports the changed lib.ts.
import { useIt } from "./direct_caller";

export function wrap(): string {
  return useIt();
}
