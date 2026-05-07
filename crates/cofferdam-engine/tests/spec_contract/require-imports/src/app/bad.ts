// VIOLATION: app file imports something else from `lib` but never the
// required logger. The rule fires once per file (not per non-matching
// import) and points at the first import.
import { format } from '../lib/format';

export function handleBad(s: string) {
  return format(s);
}
