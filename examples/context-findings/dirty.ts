export function legacy() {
  if (1 == 1) {
    return true;
  }
}

export function touched(a: number, b: number) {
  console.log("debug");
  return a + b;
}
