// Violates SeoImgMissingAlt only: <img> with no `alt`. Has a `metadata`
// export with a non-empty description, so SeoMissingMetadataExport and
// SeoNonEmptyDescription do NOT fire.

import { goodDescription as description } from "../constants";

export const metadata = { title: "Widgets", description: description };

export default function Page() {
  return <img src="/widget.png" />;
}
