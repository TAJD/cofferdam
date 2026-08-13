// Imported by sample.ts so the smoke project spans more than one file —
// `Widget.status`'s literal-union type is resolved cross-file by the
// ts-morph Project, which is what makes flaggedCrossFile a genuine
// project-wide resolution test.
export type Status = "active" | "inactive" | "pending";

export interface Widget {
  id: string;
  status: Status;
}

export function makeWidget(id: string): Widget {
  return { id, status: "active" };
}
