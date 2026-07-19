// Fixture for Consistency.ErrorHandlingIdiom.
export function parseSettings(raw: string): unknown {
  if (!raw) {
    throw new Error("settings are empty");
  }
  return JSON.parse(raw);
}
