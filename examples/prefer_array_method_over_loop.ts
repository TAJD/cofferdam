// Fixture for Refactor.PreferArrayMethodOverLoop.

// Flag — inline push, could be `.map()`.
export function doubleAll(nums: number[]) {
  const doubled: number[] = [];
  for (const n of nums) {
    doubled.push(n * 2);
  }
  return doubled;
}

// Flag — computed-then-push, could be `.map()`.
export function labelAll(nums: number[]) {
  const labels: string[] = [];
  for (const n of nums) {
    const label = `n=${n}`;
    labels.push(label);
  }
  return labels;
}

// Flag — if-gated push, could be `.filter()`.
export function evensOnly(nums: number[]) {
  const evens: number[] = [];
  for (const n of nums) {
    if (n % 2 === 0) {
      evens.push(n);
    }
  }
  return evens;
}

// Not flagged — early break disqualifies the shape.
export function firstFewEvens(nums: number[], limit: number) {
  const evens: number[] = [];
  for (const n of nums) {
    if (evens.length >= limit) {
      break;
    }
    if (n % 2 === 0) {
      evens.push(n);
    }
  }
  return evens;
}

// Not flagged — two accumulators, not a single push.
export function splitByParity(nums: number[]) {
  const evens: number[] = [];
  const odds: number[] = [];
  for (const n of nums) {
    if (n % 2 === 0) {
      evens.push(n);
    } else {
      odds.push(n);
    }
  }
  return { evens, odds };
}

// Not flagged — side effect beyond the single push.
export function loggedCopy(nums: number[]) {
  const copy: number[] = [];
  for (const n of nums) {
    console.log(n);
    copy.push(n);
  }
  return copy;
}

// Not flagged — a classic index-counting loop, no push at all.
export function sum(nums: number[]) {
  let total = 0;
  for (let i = 0; i < nums.length; i++) {
    total += nums[i];
  }
  return total;
}
