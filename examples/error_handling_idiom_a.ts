// Fixture for Consistency.ErrorHandlingIdiom.
export function parseConfig(raw: string): unknown {
  if (!raw) {
    throw new Error("config is empty");
  }
  return JSON.parse(raw);
}
