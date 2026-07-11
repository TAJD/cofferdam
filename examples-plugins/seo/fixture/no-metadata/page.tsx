// Violates SeoMissingMetadataExport only: no `metadata`/`generateMetadata`
// export. Its <img> has `alt`, so SeoImgMissingAlt does NOT fire. It
// doesn't reference `description`, so SeoNonEmptyDescription does NOT
// fire either.

export default function Page() {
  return <img src="/widget.png" alt="A widget" />;
}
