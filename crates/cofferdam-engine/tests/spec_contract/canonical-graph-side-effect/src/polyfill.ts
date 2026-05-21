// Side-effect work for the runtime — does its job at import time.
(globalThis as any).__polyfilled = true;

// Named export that NO file references by name. The only incoming
// graph edge into this file is the bare side-effect edge from
// entry.ts; OrphanExport must still flag `extraneous` because
// side-effect edges do not count as consumption.
export const extraneous = "should be flagged as orphan";
