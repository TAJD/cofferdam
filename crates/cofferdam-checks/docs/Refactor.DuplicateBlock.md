---
id: Refactor.DuplicateBlock
category: Refactor
base_priority: 12
default_severity: Medium
options: [min_statements, min_chars, include_tokens, include_ast]
---

Runs of statements that recur (after rename canonicalisation) in multiple files. Likely copy-paste — extract a shared helper. Canonicalisation maps identifier tokens to per-window local indices so renamed copies still match. Minimum window is `6` consecutive statements (and `80` characters) to keep noise low. Cross-file: per-file `run` writes fingerprints into the shared corpus; `finalize` groups by hash and emits one `Issue` per duplicate set with `related` spans pointing at every other occurrence.

```ts
// src/orders.ts:42
const items = parseItems(input);
const validated = validateItems(items);
const priced = priceItems(validated, currency);
const taxed = applyTax(priced, region);
const total = sumItems(taxed);
return { items: taxed, total };
```

```ts
// src/quotes.ts:88 — same shape, renamed: flagged as related
const products = parseItems(input);
const checkedProducts = validateItems(products);
const pricedProducts = priceItems(checkedProducts, currency);
const taxedProducts = applyTax(pricedProducts, region);
const total = sumItems(taxedProducts);
return { items: taxedProducts, total };
```

```ts
// fix: extract once
export function pipeline(input: RawInput, currency: Currency, region: Region) {
  const items = parseItems(input);
  const validated = validateItems(items);
  const priced = priceItems(validated, currency);
  const taxed = applyTax(priced, region);
  return { items: taxed, total: sumItems(taxed) };
}
```

**Suppressing:** each duplicate group is emitted as a single `Issue` covering every
occurrence — one primary location plus `related` spans for the rest. A
`cofferdam-ignore: Refactor.DuplicateBlock` comment placed at *any* occurrence (the
primary one or any related one) suppresses the whole finding, not just that copy.
You don't need to find and suppress every occurrence individually — one ignore
comment on either side of a duplicated pair is enough.
