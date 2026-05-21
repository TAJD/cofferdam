// VIOLATION: UI file imports 'localStorage' — the scripted invariant
// forbids this exact specifier from any file matching 'ui/**'.
import { read } from 'localStorage';

export function Dashboard() {
  return read('key');
}
