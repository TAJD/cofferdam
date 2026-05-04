import { liveHelper, deadHelper } from './dead_export_producer';

// `deadHelper` is imported above but never referenced anywhere — classic
// post-refactor residue. `liveHelper` is genuinely consumed.
console.log(liveHelper());
