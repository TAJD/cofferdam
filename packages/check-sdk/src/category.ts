// Mirror of cofferdam_core::Category. The Rust enum serializes as the
// lowercase variant ("warning", "design", ...); we hold both shapes so
// authors can write `Category.Warning` for ergonomics and the engine
// always reads the wire form.

export const Category = {
  Consistency: "consistency",
  Design: "design",
  Readability: "readability",
  Refactor: "refactor",
  Warning: "warning",
} as const;

export type Category = (typeof Category)[keyof typeof Category];
