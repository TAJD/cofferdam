// Fixture for Context.BlastRadius — a test file that directly calls
// the changed exported symbol `doThing`. Must be surfaced and
// annotated as a test file reaching the change.
import { doThing } from "./lib";

test("doThing stringifies", () => {
  if (doThing(1) !== "1") {
    throw new Error("unexpected value");
  }
});
