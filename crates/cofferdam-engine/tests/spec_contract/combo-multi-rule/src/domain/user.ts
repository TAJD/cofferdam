import { fetchFromDb } from '../infra/db/connection';

export type UserId = string;

export async function fetchUser(id: UserId) {
  return fetchFromDb(id);
}
