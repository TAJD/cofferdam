// Fixture for Refactor.PurityHeuristic.
// This check ships opt-in (disabled by default) — see cofferdam.toml.

// Flag — both functions read module-level `requestCount` (which IS
// reassigned in this file) without it being covered by their own
// parameters: `logRequest` reads it directly, and `bumpRequestCount`
// reads it implicitly via the compound `+=` (read-then-write).
let requestCount = 0;

export function logRequest(name: string) {
  console.log(`${name}: request #${requestCount}`);
}

export function bumpRequestCount() {
  requestCount += 1;
}

// Not flagged — read-only module state (never reassigned anywhere in
// the file): a config object.
let config = { apiUrl: "https://api.example.com" };

export function fetchData() {
  return fetch(config.apiUrl);
}

// Not flagged — read-only module state: a logger instance.
let logger = console;

export function reportError(message: string) {
  logger.error(message);
}

// Not flagged — read-only module state: a memoization cache that's
// mutated only via its own methods (`.set`), never reassigned itself.
let cache = new Map<string, number>();

export function memoizedSquare(n: number): number {
  if (cache.has(n)) {
    return cache.get(n)!;
  }
  const result = n * n;
  cache.set(n, result);
  return result;
}

// Not flagged — `total` is module-level and reassigned elsewhere, but
// this function's own parameter (same name) covers it.
let total = 0;

export function resetTotal() {
  total = 0;
}

export function addToTotal(total: number) {
  return total + 1;
}
