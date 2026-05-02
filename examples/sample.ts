// A sample file with one deliberately-too-long line to exercise the
// Readability.MaxLineLength check end-to-end.

export function shortFunction() {
  return 1;
}

export const someVeryLongVariableNameThatExceedsTheCharacterLimit = "this whole line is intentionally well past the 120-character cofferdam default";

export function ok() {
  return 2;
}
