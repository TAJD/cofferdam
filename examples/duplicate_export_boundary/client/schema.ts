// Client half of an intentional cross-boundary mirror. With
//
//   [checks."Design.DuplicateExportName"]
//   exempt_boundary_pairs = ["client/**|server/**"]
//
// the `UserId` / `parseUser` collision with ../server/schema.ts is exempt.
// `formatUser` still collides with ../internal/legacy.ts, which is NOT on a
// declared side, so that finding survives the exemption.

export const UserId = "user-id";

export function parseUser(raw: string) {
  return JSON.parse(raw);
}

export function formatUser(name: string) {
  return name.trim();
}
