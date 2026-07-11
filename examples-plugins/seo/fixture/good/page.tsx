// Fully compliant: has a `metadata` export with a non-empty description
// sourced from `goodDescription`, and its <img> has `alt`. None of the
// 3 .tsx-scoped checks fire against this file.

import { goodDescription as description } from "../constants";

export const metadata = { title: "Widgets", description: description };

export default function Page() {
  return <img src="/widget.png" alt="A widget" />;
}
