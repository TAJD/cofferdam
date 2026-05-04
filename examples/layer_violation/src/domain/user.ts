// OK: domain may import from infra.
import { connect } from '../infra/connection';

export interface User {
  id: string;
}

export function newUser(id: string): User {
  connect();
  return { id };
}
