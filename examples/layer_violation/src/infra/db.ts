// Bad: infra is isolated and must not depend on domain.
import { User } from '../domain/user';

export function persist(user: User): void {
  void user;
}
