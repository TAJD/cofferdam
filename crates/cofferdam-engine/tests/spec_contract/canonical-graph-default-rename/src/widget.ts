// Default export consumed below via `import { default as W }`. The
// canonical graph records this as a Named edge with
// `source_name == "default"`; OrphanExport's incoming walk reroutes
// it into the default-consumption bucket so this export is NOT
// flagged.
export default function Widget(id: string): string {
  return `widget-${id}`;
}
