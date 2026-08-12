// Minimal fixture with exactly one deterministic finding
// (Design.MaxParameters, medium severity) — used by release smoke
// tests (scripts/smoke-install.{sh,ps1}) and cofferdam-mcp's
// round-trip tests to confirm the installed binary actually runs
// checks end-to-end, independent of which specific check fires.
function tooManyParams(a: number, b: number, c: number, d: number, e: number, f: number) {
  return a + b + c + d + e + f;
}
