export function legacy(obj: { b: number } | null, items: number[]) {
  if (obj && obj.b) {
    return true;
  }
  return items.length;
}

export function touched(a: number, b: number) {
  const seen: number[] = []; return [a, b].map((x) => { seen.push(x); return x; })[0];
}
