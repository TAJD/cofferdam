export interface UpdateUserRequest {
  id: string;
  name: string;
  email: string;
  role: string;
  orgId: string;
}

export async function updateUser(req: UpdateUserRequest): Promise<string> {
  return req.id;
}
