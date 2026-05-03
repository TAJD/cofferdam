# `@cofferdam-fixtures/brand-casing`

End-to-end fixture for the cofferdam plugin SDK (cd-7e4). Ports the
`BrandCasing` rovikore-host Credo check to `@cofferdam/check-sdk` and runs it
through the napi `worker_threads` loader against `fixture.ts`.

**Status: design-only.** The SDK epic (cd-81a) has no completed children
yet; only `fixture.ts` (and the design doc) are committed. Once the SDK
lands, `package.json`, `tsconfig.json`, `src/index.ts`, and `expected.json`
fill in to match the spec in `docs/plugin-sdk-e2e.md`.

## What this fixture proves (when complete)

1. A plugin written with `defineCheck` from `@cofferdam/check-sdk` loads via
   `cofferdam.toml`'s `plugins = [...]` array.
2. The plugin's `LineView` walk classifies comments, doc comments, and
   string literals correctly (cd-81a.1).
3. Both plugin-level (`// brand:ignore`) and engine-level
   (`// cofferdam-ignore: BrandCasing`) suppression coexist (cd-81a.4 +
   cd-cmb).
4. Issue spans round-trip back to the original byte offsets — `source.slice
   (byte_start, byte_end)` returns the literal trigger word
   (`scripts/check-spans.mjs`, cd-n14).
5. The negative `index.fail.ts` fixture fails `tsc` — the SDK's types are
   tight enough to catch a wrong AST property access at compile time
   (cd-81a.2 / cd-81a.8).

## Running locally (when complete)

```bash
pnpm --filter brand-casing build
cargo run --release -p cofferdam-cli -- check \
  examples-plugins/brand-casing/fixture.ts --format json > actual.json
node scripts/check-spans.mjs actual.json examples-plugins/brand-casing/fixture.ts Rovikore
diff -u expected.json actual.json
```

## Expected output

Exactly 2 issues. See `fixture.ts` — the lines marked `FLAG #1` and `FLAG #2`.
Every other occurrence of the trigger is exempted by one of the rules
documented in `docs/plugin-sdk-e2e.md` §1.

## Design source

Full spec: `docs/plugin-sdk-e2e.md` (in this repo).

Original Credo check: `C:/Users/tajdi/rovikore-host/backend/dev_checks/rovikore_host_credo/brand_casing.ex`.
