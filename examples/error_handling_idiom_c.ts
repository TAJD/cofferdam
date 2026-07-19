// Fixture for Consistency.ErrorHandlingIdiom.
export function parseProfile(raw: string): unknown {
  if (!raw) {
    throw new Error("profile is empty");
  }
  return JSON.parse(raw);
}
