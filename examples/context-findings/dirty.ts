export function legacy(obj: { b: number } | null) {
  if (obj && obj.b) {
    return true;
  }
}

export function touched(a: number, b: number) {
  const unused = 5;
  return a + b;
}
