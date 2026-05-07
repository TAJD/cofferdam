// VIOLATION: Design.InvariantViolation — app layer must not import
// directly from src/infra/db.
import { fetchFromDb } from '../infra/db/connection';

export function renderPage(id: string) {
  return fetchFromDb(id);
}
