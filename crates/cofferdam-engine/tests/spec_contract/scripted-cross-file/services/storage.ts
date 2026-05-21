// Outside ui/** — the `when` gate filters this file out even though
// it imports localStorage. Service-layer code is allowed to.
import { read } from 'localStorage';

export function loadKey(k: string) {
  return read(k);
}
