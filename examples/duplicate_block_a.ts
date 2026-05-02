// Pair with duplicate_block_b.ts. Both files contain a 6-statement
// run with the same shape but renamed identifiers. Refactor.DuplicateBlock
// should flag it. The unique pre/post statements should NOT be involved.

export function fetchUserOrders(userId: string): Promise<unknown> {
  // unique preamble
  console.log("a:start");

  // ↓ duplicated 6-statement run begins
  const startTime = Date.now();
  const cacheKey = `orders:${userId}`;
  const cached = readCache(cacheKey);
  if (cached !== null && Date.now() - cached.at < 5000) {
    return Promise.resolve(cached.value);
  }
  const result = doFetch(`/api/users/${userId}/orders`);
  const duration = Date.now() - startTime;
  // ↑ duplicated 6-statement run ends

  // unique postamble
  console.log("a:done", duration);
  return result;
}

declare function readCache(k: string): { at: number; value: unknown } | null;
declare function doFetch(url: string): Promise<unknown>;
