// OK: domain → infra is allowed.
import { connect } from '../infra/connection';

export interface User {
  id: string;
}

export function newUser(id: string): User {
  connect();
  return { id };
}
