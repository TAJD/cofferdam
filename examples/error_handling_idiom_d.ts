// Fixture for Consistency.ErrorHandlingIdiom.
export function parsePreferences(raw: string): unknown {
  if (!raw) {
    throw new Error("preferences are empty");
  }
  return JSON.parse(raw);
}
