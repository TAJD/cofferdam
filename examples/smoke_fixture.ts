// Minimal fixture with exactly one deterministic finding
// (Design.ReadonlyArrayParam, low severity) — used by release smoke
// tests (scripts/smoke-install.{sh,ps1}) and cofferdam-mcp's
// round-trip tests to confirm the installed binary actually runs
// checks end-to-end, independent of which specific check fires.
function sumAll(items: number[]) {
  return items.reduce((total, n) => total + n, 0);
}
