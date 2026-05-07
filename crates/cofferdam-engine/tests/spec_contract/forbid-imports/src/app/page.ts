// VIOLATION: app layer must not import from src/infra/db (the rule
// forbids this prefix). Direct service-layer access would be fine.
import { fetchFromDb } from '../infra/db/connection';

export function renderPage(id: string) {
  return fetchFromDb(id);
}
