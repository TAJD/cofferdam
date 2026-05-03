// Two named exports — only `usedHelper` has a consumer.
export function usedHelper(): string {
  return 'hello';
}

// Should be flagged: declared but no other file imports it.
export function unusedHelper(): number {
  return 42;
}

// Default export with no consumer — also flagged.
export default function orphanDefault(): boolean {
  return true;
}
