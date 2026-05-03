// Cycle: a → b → c → a
import { fromB } from './import_cycle_b';

export function fromA(): string {
  return fromB() + 'A';
}
