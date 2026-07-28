import { covered } from "./covered";
import { ValidationError } from "./errors";

test("covered returns 1", () => {
  if (covered() !== 1) {
    throw new Error("unexpected value");
  }
});

test("ValidationError keeps its type identity", () => {
  const err: unknown = new ValidationError("bad");
  expect(err instanceof ValidationError).toBe(true);
  expect(() => {
    throw new ValidationError("bad");
  }).toThrow(ValidationError);
});
