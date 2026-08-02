// New file added to the handlers/ module — has no shape of its own yet.
// `cofferdam context` should still surface the established
// CreateUserRequest/UpdateUserRequest/ListUsersRequest convention from
// its siblings.
export async function deleteUser(id: string): Promise<void> {
  void id;
}
