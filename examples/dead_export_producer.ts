// `liveHelper` is referenced by the consumer; `deadHelper` is imported
// but never referenced after the import statement.
export function liveHelper(): number {
  return 1;
}

export function deadHelper(): number {
  return 2;
}
