// Not on either side of the declared boundary. Its `formatUser` collides
// with ../client/schema.ts and must stay flagged even with
// exempt_boundary_pairs configured.

export function formatUser(name: string) {
  return name.toUpperCase();
}
