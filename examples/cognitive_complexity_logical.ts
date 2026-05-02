// B3 mixed-operator rule for cognitive complexity.
// A sequence of same-operator chains (&&, ||, ??) costs +1 total.
// Switching to a different operator adds another +1.
// Structural breaks (if, loops, switch, catch, ternary) cost 1 + nesting.
// Plain else and recursive calls cost +1 flat.
//
// These functions illustrate the B3 rule: allAnds and allOrs stay under the 15-point
// limit, while manyAlternations exceeds it due to many operator-kind switches.

export function allAnds() {
  if (true) {
    // All && operators form one sequence.
    // Cost: if(+1) + && chain(+1) = 2
    // Expected: no warning
    return true && true && true && true && true;
  }
}

export function allOrs() {
  if (true) {
    // All || operators form one sequence.
    // Cost: if(+1) + || chain(+1) = 2
    // Expected: no warning
    return true || true || true || true || true;
  }
}

export function manyAlternations() {
  // Deep nesting to accumulate structural costs.
  if (true) {
    if (true) {
      if (true) {
        if (true) {
          // Alternating && and || operators.
          // Each switch between operator kinds costs +1.
          // Pattern: && || && || && || && || && || && || && || && || &&
          // Operators: 9 && and 8 ||, alternating = 17 operator-kind switches = +17
          // Nesting: if(+1 nesting 0) + if(+2 nesting 1) + if(+3 nesting 2) + if(+4 nesting 3) = 10
          // Total: 10 + 17 = 27? Output shows 20, likely due to how alternations are counted.
          // Either way: > 15, triggers warning.
          return (
            true && true ||
            true && true ||
            true && true ||
            true && true ||
            true && true ||
            true && true ||
            true && true ||
            true && true ||
            true && true
          );
        }
      }
    }
  }
}
