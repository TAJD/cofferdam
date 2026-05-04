// Barrel re-exports both. Only `alpha` is consumed downstream — `beta`
// is a dead re-export that tsc's noUnusedLocals would let through.
export { alpha } from './unused_import_origin';
export { beta } from './unused_import_origin';
