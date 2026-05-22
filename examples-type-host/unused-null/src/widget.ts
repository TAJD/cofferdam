// Imported by sample.ts so the smoke project spans more than one file —
// `Widget.id` is resolved cross-file by the ts-morph Project, which is
// what makes flaggedCrossFile a genuine project-wide resolution test.
export interface Widget {
  id: string;
  label: string;
}

export function makeWidget(id: string): Widget {
  return { id, label: id };
}
