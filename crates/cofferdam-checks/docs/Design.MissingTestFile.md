---
id: Design.MissingTestFile
category: Design
base_priority: 4
default_severity: Low
options:
  - name: test_match_patterns
    kind: string_list
    default: ["{name}.test.ts", "{name}.test.tsx", "{name}.spec.ts", "{name}.spec.tsx", "__tests__/{name}.test.ts", "__tests__/{name}.test.tsx", "__tests__/{name}.spec.ts", "__tests__/{name}.spec.tsx"]
  - name: test_file_patterns
    kind: string_list
    default: [".test.", ".spec.", "_test.", "_spec.", "/__tests__/", "/__mocks__/"]
  - name: framework_entry_patterns
    kind: string_list
    default: ["/page.", "/layout.", "/route.", "..."]
---

A file exports at least one real (non-type-only, non-re-export) symbol but no corresponding test file exists anywhere in the project.

```ts
// format-currency.ts — exported behavior with no test file anywhere
export function formatCurrency(cents: number): string {
  return `$${(cents / 100).toFixed(2)}`;
}
```

Not flagged — the file only re-exports (a barrel) or only exports types, so there's no behavior of its own to test:

```ts
// index.ts
export * from "./format-currency";
export type { CurrencyOptions } from "./types";
```

Matching: for each candidate source file, the check substitutes `{name}` (the file's stem) into every `test_match_patterns` template, resolves the result relative to the source file's own directory, and looks for it in the project's known-files universe (built from the import/export graph, same construction `Design.ImportFanOutOutlier` uses). The default templates cover same-directory and sibling-`__tests__/` conventions for both `.test.` and `.spec.` naming, across `.ts`/`.tsx`/`.js`/`.jsx`/`.mjs`/`.mts`/`.cts` extensions. Configure `test_match_patterns` for a project with a different layout (e.g. a top-level `tests/` directory mirroring `src/`).

Type-identity escape hatch: a file is also treated as tested when a test file imports one of its real exports and uses that binding in a *type-assertion* position — `err instanceof ValidationError`, `expect(fn).toThrow(ValidationError)`, `expect(err).toBeInstanceOf(ValidationError)`. Error and exception classes are routinely tested for their type identity from the test of whatever *throws* them, rather than from a test file named after the class's own module, and the filename rule alone reports those as untested. The credit is deliberately narrow: merely importing a binding into a test (a fixture builder, say) does not count, the asserted local name must resolve to a real export of the file, and resolution follows one re-export hop so a class reached through an `errors/index.ts` barrel still credits `errors/validation.ts`. There is no option for this — it removes false positives only, so there is nothing to tune.

Exemptions: a file already recognised as a test/mock itself (`test_file_patterns`, same default list as `Design.OrphanExport`) is never flagged for missing its own test. A framework entry-point file (`framework_entry_patterns`, shared default with `Design.OrphanExport`/`Warning.UnusedImport`) is exempt too — the framework runtime, not a unit test, is what exercises it.

Scope: the known-files universe only includes files that appear as an import source, an export site, or a resolved import target — a test file with zero imports and zero exports of its own wouldn't be visible here even if it exists on disk. In practice a test file almost always imports the module it's testing, so this is a narrow gap. A project with a `tests/` directory that mirrors `src/` at the project root (rather than sitting alongside each source file) needs a custom `test_match_patterns` entry — the defaults only check same-directory and sibling-`__tests__/` locations. The default templates aren't matched against the source file's own extension — they're just a flat list covering every supported extension — so a `.ts` source with only a stray `.test.js` sibling would technically also suppress a finding; this cross-extension false-negative is accepted as harmless in practice.
