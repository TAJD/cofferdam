// Fixture for Design.UnionExhaustivenessGap (CD-118).
//
// This check is TYPE-AWARE: it only fires when cofferdam runs with the
// ts-morph type host (a project with tsconfig.json + ts-morph installed
// and `[engine] type_aware` enabled). Running `cofferdam check` on this
// file inside the cofferdam repo (a Rust project, no type host) produces
// NO findings — that's expected. The decision logic is pinned by unit
// tests with a stub oracle in `cofferdam-checks/src/design/mod.rs`, and
// the end-to-end path by a gated test against a real ts-morph project.

type Shape =
  | { kind: "circle"; radius: number }
  | { kind: "square"; side: number }
  | { kind: "triangle"; base: number; height: number };

// --- flagged: missing variant, no default ------------------------------

export function area(shape: Shape) {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius ** 2;
    case "square":
      return shape.side ** 2;
  }
}

// --- not flagged: every variant handled --------------------------------

export function areaComplete(shape: Shape) {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius ** 2;
    case "square":
      return shape.side ** 2;
    case "triangle":
      return (shape.base * shape.height) / 2;
  }
}

// --- not flagged: default case is treated as an intentional catch-all --

export function areaWithDefault(shape: Shape) {
  switch (shape.kind) {
    case "circle":
      return Math.PI * shape.radius ** 2;
    default:
      return 0;
  }
}

// --- not flagged: plain string discriminant, not a literal union -------

export function describe(kind: string) {
  switch (kind) {
    case "circle":
      return "round";
  }
}
