// Fixture for Refactor.CognitiveComplexity recursion penalty (Sonar B4).
// Direct recursive calls (function calling itself by name) add +1 flat to complexity.
// This demonstrates both a flagging recursive function and a non-flagging iterative equivalent.

// Recursive function with deep nesting and multiple recursive calls.
// Complexity is increased through nested conditionals and recursive calls.
// Scoring:
//   - Level 1: if (n <= 0) +1
//   - Level 1: if inside else +1 (flat, not nested)
//   - Level 2: nested if (n === 1) +1+1 = +2
//   - Level 1: if (n > 100) +1
//   - Level 2: nested if (n > 10000) +1+1 = +2
//   - Level 2: nested if-else (+1+1 for else) = +2
//   - 2x recursive calls: +1 each = +2 flat
//   Total: 1 + 1 + 2 + 1 + 2 + 2 + 2 = ~12, plus more with deeper nesting
function complexRecursiveFib(n: number): number {
  if (n <= 0) {
    return 0;
  } else if (n === 1) {
    if (n === 1) {
      return 1;
    }
  }

  if (n > 100) {
    if (n > 10000) {
      return 0;
    } else if (n > 1000) {
      return -1;
    } else {
      return 1;
    }
  }

  if (n === 2) {
    return 1;
  } else if (n === 3) {
    return 2;
  } else if (n > 3 && n < 50) {
    if (n % 2 === 0) {
      return complexRecursiveFib(n - 1) + complexRecursiveFib(n - 2);
    } else if (n % 3 === 0) {
      return complexRecursiveFib(n - 1);
    }
  } else {
    return complexRecursiveFib(n - 1) + complexRecursiveFib(n - 2) + complexRecursiveFib(n - 3);
  }

  return complexRecursiveFib(n - 1);
}

// Tree traversal with recursive calls and multiple branches.
// This version explicitly has higher complexity through branching.
function complexRecursion(n: number, depth: number = 0): number {
  if (depth > 5) {
    return 0;
  }
  if (n === 0) {
    return 1;
  }
  if (n < 0) {
    return -1;
  }
  if (n > 1000) {
    return 0;
  }

  let result = 0;
  if (n % 2 === 0) {
    result = complexRecursion(n - 1, depth + 1);
  } else if (n % 3 === 0) {
    result = complexRecursion(n - 2, depth + 1) + complexRecursion(n - 3, depth + 1);
  } else {
    result = complexRecursion(n - 1, depth + 1);
  }

  return result;
}

// Non-recursive iterative equivalent: should NOT flag.
// Uses a loop instead of recursion, lower complexity.
function iterativeFib(n: number): number {
  if (n <= 0) {
    return 0;
  }
  if (n === 1) {
    return 1;
  }

  let prev = 0;
  let curr = 1;

  for (let i = 2; i <= n; i++) {
    const next = prev + curr;
    prev = curr;
    curr = next;
  }

  return curr;
}

// Arrow function with recursion-by-reference (through variable assignment).
// Arrows don't have names, so self-reference via variable doesn't count as
// named recursion. This is intentionally NOT flagged as recursive by the check.
const recursiveArrow: (n: number) => number = (n: number) => {
  if (n <= 0) {
    return 1;
  }
  if (n > 100) {
    return 0;
  }
  // This calls via variable reference, not by name, so no recursion penalty.
  return recursiveArrow(n - 1);
};
