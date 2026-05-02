// Cognitive complexity flags deeply-nested code MORE than flat-but-long
// branching. Default limit: 15.
//
// `flatSwitch` has many branches but no nesting → should NOT flag.
// `deeplyNested` has fewer branches but stacked nesting → SHOULD flag.

// Many cases, no nesting. Cognitive ~5 (each case +1, no penalty).
export function flatSwitch(mode: string): string {
  switch (mode) {
    case "a":
      return "A";
    case "b":
      return "B";
    case "c":
      return "C";
    case "d":
      return "D";
    case "e":
      return "E";
    default:
      return "?";
  }
}

// Three nested ifs inside a for inside an if. Cognitive ≈ 18:
//   if (1+0) → +1
//     for (1+1) → +2
//       if (1+2) → +3
//         while (1+3) → +4
//           if (1+4) → +5
//             if (1+5) → +6 ... already over 15.
export function deeplyNested(items: number[], mode: string): number {
  if (mode === "process") {
    for (const item of items) {
      if (item > 0) {
        let i = item;
        while (i > 0) {
          if (i % 2 === 0) {
            if (i > 100) {
              i = i / 2;
            } else {
              i = i - 1;
            }
          } else {
            i = i + 1;
          }
        }
      }
    }
  }
  return items.length;
}
