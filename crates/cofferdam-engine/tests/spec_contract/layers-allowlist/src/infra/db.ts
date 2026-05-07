// VIOLATION: infra is isolated and must not import from domain.
import { User } from '../domain/user';

export function persist(user: User): void {
  void user;
}
