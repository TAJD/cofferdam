export interface CreateUserRequest {
  name: string;
  email: string;
  role: string;
  orgId: string;
}

export async function createUser(req: CreateUserRequest): Promise<string> {
  return req.name;
}
