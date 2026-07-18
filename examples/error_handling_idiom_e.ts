// Fixture for Consistency.ErrorHandlingIdiom.
//
// The project (a-d.ts) predominantly throws for errors. This file
// returns an error-shaped value instead — the minority idiom, flagged.
export function parseUser(raw: string): unknown {
  if (!raw) {
    return { error: "user payload is empty" };
  }
  // Not flagged — `{ error: null }` is the success arm of a
  // Result-shaped return, not a competing error idiom.
  return { error: null, value: JSON.parse(raw) };
}
