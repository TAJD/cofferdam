// Lone file in its own directory — no siblings, so no established
// pattern can exist here. `cofferdam context` on this file must emit
// no Context.Precedent item.
export function formatName(name: string): string {
  return name.trim();
}
