import { covered } from "./covered";

test("covered returns 1", () => {
  if (covered() !== 1) {
    throw new Error("unexpected value");
  }
});
