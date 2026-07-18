// Fixture for Design.DuplicateTypeShape.
//
// Not flagged — too few fields (below MIN_FIELDS) to distinguish real
// duplication from coincidence, even though Point and Size below share
// an identical shape.
export interface Point {
  x: number;
  y: number;
}

export interface Size {
  x: number;
  y: number;
}

// Not flagged — shapes diverge enough (title/body vs title/publishedAt)
// to be different types.
export type Draft = {
  id: string;
  title: string;
  body: string;
};

export type Published = {
  id: string;
  title: string;
  publishedAt: string;
};
