// Violates SeoNonEmptyDescription only: `description` resolves via
// resolveLiteral to `badDescription`'s empty-string literal in
// ../constants.ts. Has a `metadata` export (SeoMissingMetadataExport
// does NOT fire) and its <img> has `alt` (SeoImgMissingAlt does NOT
// fire).
//
// NOTE: SeoNonEmptyDescription matches `IdentifierReference` nodes
// named literally "description" — the import alias below keeps the
// use-site name in sync with that check's matched identifier name.
// Uses an explicit `description: description` property rather than
// the shorthand `{ description }` form: ts-morph's symbol resolution
// for a shorthand property assignment resolves to the property's own
// symbol rather than following the alias to the imported binding, so
// `resolveLiteral` can't see through it (a pre-existing gap in
// type-host-core.mjs's CD-82 resolution, out of scope for CD-86).

import { badDescription as description } from "../constants";

export const metadata = { title: "Widgets", description: description };

export default function Page() {
  return <img src="/widget.png" alt="A widget" />;
}
