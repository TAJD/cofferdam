import { fromA } from './import_cycle_a';

export function fromC(): string {
  return fromA() + 'C';
}
