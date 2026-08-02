export interface ListUsersRequest {
  orgId: string;
  role: string;
  name: string;
  email: string;
}

export async function listUsers(req: ListUsersRequest): Promise<string[]> {
  return [req.orgId];
}
