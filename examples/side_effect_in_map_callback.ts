// Fixture for Refactor.SideEffectInMapCallback.

// Flag — mutates outer-scope `seen` inside a .map callback.
const items: number[] = [1, 2, 3];
const seen: number[] = [];
export const doubled = items.map((item) => {
  seen.push(item);
  return item * 2;
});

// Flag — calls console.* inside a .filter callback.
export const positives = items.filter((item) => {
  console.log("checking", item);
  return item > 0;
});

// Not flagged — only mutates/declares its own locals.
export const scaled = items.map((item) => {
  const local = item * 2;
  return local;
});

// Not flagged — a nested function's own mutation of its own local
// doesn't count against the outer callback.
export const wrapped = items.map((item) => {
  function makeLabel() {
    let label = "";
    label += String(item);
    return label;
  }
  return makeLabel();
});

// Not flagged — return value discarded, but no other side effect; a
// distinct "use forEach instead" concern, out of scope for this check.
export function processAll() {
  items.map((item) => process(item));
}

// Not flagged — .forEach is exempt; a side effect there is the point.
items.forEach((item) => {
  seen.push(item);
});

// Flag — writes into an outer-scope object property, not just an
// array-mutating method call.
const state = { count: 0 };
export const withCount = items.map((item) => {
  state.count += 1;
  return item;
});

// Flag — writes into an outer-scope array by index.
const acc: number[] = new Array(items.length);
export const withIndexWrite = items.map((item, i) => {
  acc[i] = item;
  return item;
});

declare function process(_item: number): number;
