// Fixture for Context.BlastRadius — depth-1 caller of the changed
// exported symbol `doThing`.
import { doThing } from "./lib";

export function useIt(): string {
  return doThing(1);
}
