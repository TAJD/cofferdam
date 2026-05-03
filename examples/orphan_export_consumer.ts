// Imports `usedHelper` from the producer; `unusedHelper` is left dangling.
import { usedHelper } from './orphan_export_producer';

console.log(usedHelper());
