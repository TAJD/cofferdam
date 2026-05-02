// Pair with duplicate_block_a.ts. Identical 6-statement run with
// renamed identifiers — should still be detected after canonicalisation.

export function fetchInvoiceItems(invoiceId: string): Promise<unknown> {
  // different preamble (single statement)
  void invoiceId;

  // ↓ duplicated 6-statement run begins (renamed)
  const t0 = Date.now();
  const key = `items:${invoiceId}`;
  const hit = readCache(key);
  if (hit !== null && Date.now() - hit.at < 5000) {
    return Promise.resolve(hit.value);
  }
  const out = doFetch(`/api/invoices/${invoiceId}/items`);
  const elapsed = Date.now() - t0;
  // ↑ duplicated 6-statement run ends

  return Promise.race([out, Promise.resolve(elapsed)]);
}

declare function readCache(k: string): { at: number; value: unknown } | null;
declare function doFetch(url: string): Promise<unknown>;
