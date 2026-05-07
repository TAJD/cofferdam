// OK: `services` is not in the rule's `from_layers` filter, so the
// same forbidden prefix is allowed here.
import { fetchFromDb } from '../infra/db/connection';

export function getUser(id: string) {
  return fetchFromDb(id);
}
