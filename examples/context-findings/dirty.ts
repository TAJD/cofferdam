export function legacy(obj: { b: number } | null, items: number[]) {
  if (obj && obj.b) {
    return true;
  }
  return items.length;
}

export function touched(a: number, b: number, c: number, d: number, e: number, f: number) {
  return a + b + c + d + e + f;
}
