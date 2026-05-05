// TenantIsolation cd-11j fixture — Pattern C / stateful walk.
//
// Demonstrates three of the four cases the bead enumerates within a
// single file (host runs the check against `fixture.ts`):
//
//   1. Scoped query     → OK, no finding
//   2. Unscoped query   → FLAG
//   3. Magic-comment    → OK (suppression directive on the call)
//
// The fourth case ("unscoped query in a wrapped file → OK") needs its
// own file because the wrapper-import sentinel suppresses *every*
// finding in the file — it can't co-exist with case 2 in one fixture.
// See `wrapped-fixture.ts` (sibling file, not driven by
// scripts/check-plugin-fixtures.mjs) for that demonstration.

// Stand-in for the Prisma client. The check matches by callee shape
// (`prisma.<Model>.<queryMethod>`), not by an imported type.
declare const prisma: {
  merchant: {
    findMany(args?: unknown): Promise<unknown>;
    findFirst(args?: unknown): Promise<unknown>;
    findUnique(args: unknown): Promise<unknown>;
  };
};

// CASE 1 — scoped query. `siteId` in `where` clears the tenant check.
export async function getMerchant(id: string, siteId: string) {
  return prisma.merchant.findUnique({
    where: { id, siteId },
  });
}

// CASE 2 — unscoped query. `where` has only `active`; no tenant field.
// FLAG: line 31 (the `prisma.merchant.findMany(...)` call expression).
export async function listActiveMerchants() {
  return prisma.merchant.findMany({
    where: { active: true },
  });
}

// CASE 3 — magic-comment exempted. The author has manually verified
// scoping and asserts the call is safe. Suppression directive on the
// call's start line clears the would-be flag.
export async function listAllMerchantsAdmin() {
  // cofferdam-ignore: TenantIsolation: admin endpoint, scoping enforced by middleware
  return prisma.merchant.findMany({
    where: { active: true },
  });
}
