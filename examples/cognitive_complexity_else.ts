// This fixture demonstrates the cognitive complexity cost of plain `else` blocks.
// `withElse` uses multiple else blocks within nested structures, exceeding limit (15).
// `withoutElse` refactors to guard-clause early returns, staying under the limit.

function withElse(data: any[]): number {
  let result = 0;

  for (const item of data) {
    // Loop: 1 + nesting(0) = 1
    if (item) {
      // If: 1 + nesting(1) = 2
      for (const sub of item) {
        // Nested loop: 1 + nesting(2) = 3
        if (sub > 0) {
          // If: 1 + nesting(3) = 4
          result += sub;
        } else {
          // Plain else: 1 (flat, no nesting)
          result -= 1;
        }

        if (sub % 2 === 0) {
          // If: 1 + nesting(3) = 4
          result *= 2;
        } else {
          // Plain else: 1 (flat, no nesting)
          result += 1;
        }
      }
    } else {
      // Plain else (outer): 1 (flat, no nesting)
      result = 0;
    }
  }

  return result;
}
// Score: loop(1) + if(2) + nested_loop(3) + if(4) + else(1) + if(4) + else(1) + else(1) = 17
// Exceeds limit of 15.

function withoutElse(data: any[]): number {
  let result = 0;

  for (const item of data) {
    // Loop: 1 + nesting(0) = 1
    if (!item) {
      // If: 1 + nesting(1) = 2
      result = 0;
      continue;
    }

    for (const sub of item) {
      // Nested loop: 1 + nesting(2) = 3
      if (sub <= 0) {
        // If: 1 + nesting(3) = 4
        result -= 1;
        continue;
      }

      result += sub;

      if (sub % 2 !== 0) {
        // If: 1 + nesting(3) = 4
        result += 1;
        continue;
      }

      result *= 2;
    }
  }

  return result;
}
// Score: loop(1) + if(2) + nested_loop(3) + if(4) + if(4) = 14
// Stays under limit of 15.
