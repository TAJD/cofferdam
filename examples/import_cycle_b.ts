import { fromC } from './import_cycle_c';

export function fromB(): string {
  return fromC() + 'B';
}
