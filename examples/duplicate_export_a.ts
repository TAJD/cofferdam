// Pair with duplicate_export_b.ts: both export `parseDate` and `helper`,
// which Design.DuplicateExportName should flag. `unique_to_a` should NOT.

export function parseDate(input: string): Date {
  return new Date(input);
}

export const helper = (x: number) => x * 2;

export const unique_to_a = "only-here";

export class Foo {
  greet() {
    return "hi from A";
  }
}
