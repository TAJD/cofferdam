export function total(lineItems: number[]): number {
  return lineItems.reduce((sum, n) => sum + n, 0);
}
