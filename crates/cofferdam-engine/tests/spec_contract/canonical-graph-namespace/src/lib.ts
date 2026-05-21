// Mixed-surface module: one default export plus two named exports.
// Only consumer (consumer.ts) does `import * as Lib from './lib'`.
//
// Expected canonical-graph consumption:
//   - one ImportsAsValue edge with `import_kind == "namespace"`
//   - ns_touched = true on this file's incoming summary
//   - named exports `alpha` and `beta` shielded (silent)
//   - default export still orphan (namespace doesn't reach defaults)

export const alpha = 1;
export const beta = 2;

export default function libDefault() {
  return alpha + beta;
}
