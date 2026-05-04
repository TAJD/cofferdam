// EXPECTED to fail tsc. CI runs `pnpm --filter brand-casing build:fail`
// and asserts a non-zero exit. The @ts-expect-error directives pin the
// exact lines where the SDK's types are supposed to fire — if any of
// them ever stops failing (types loosened), tsc reports the directive
// itself as unused and this file still fails to compile, surfacing the
// regression.

import { Category, defineCheck } from "@cofferdam/check-sdk";

export default defineCheck({
  id: "BrandCasing",
  category: Category.Warning,
  basePriority: 15,
  explanation: "x",
  options: {
    brand: { default: "ROVIKORE", type: "string" },
  },
  run(file, ctx, opts) {
    // Wrong method name on the SourceFile shape — the SDK exposes
    // `lines()`, not `lyne()`. Pins cd-81a.1's API tightness.
    // @ts-expect-error
    file.lyne();

    // Wrong opts member — schema declared { brand }, not { branded }.
    // @ts-expect-error
    void opts.branded;

    // Wrong opts member type — opts.brand is string, not number.
    // @ts-expect-error
    const _n: number = opts.brand;

    // ctx.report requires `span`. Omitting it must be a type error.
    // @ts-expect-error
    ctx.report({ message: "no span" });
  },
});
