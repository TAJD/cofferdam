// Server half of the mirror declared in ../client/schema.ts. The two files
// deliberately share `UserId` and `parseUser` — that is the contract.

export const UserId = "user-id";

export function parseUser(raw: string) {
  return JSON.parse(raw) as { id: string };
}
