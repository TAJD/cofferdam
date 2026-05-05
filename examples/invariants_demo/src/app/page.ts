// VIOLATION: Design.InvariantViolation — `app` layer cannot import from src/infra/db.
import { fetchFromDb } from "../infra/db/connection";

export function renderPage(id: string) {
  return fetchFromDb(id);
}
