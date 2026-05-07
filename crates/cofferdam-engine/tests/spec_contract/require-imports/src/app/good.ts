// OK: imports the required logger module — resolves under
// `src/lib/logger/`, satisfying the directory-prefix check.
import { log } from '../lib/logger';

export function handleGood() {
  log('hi');
}
