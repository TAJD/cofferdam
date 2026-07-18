// Fixture for Refactor.MutatedParameter.

// Flag — direct reassignment of a parameter.
export function withDiscount(price: number, rate: number) {
  price = price - price * rate;
  return price;
}

// Flag — property write on an object parameter.
export function tagAsProcessed(item: { status: string }) {
  item.status = "processed";
  return item;
}

// Flag — index write on an array parameter.
export function zeroFirst(values: number[]) {
  values[0] = 0;
  return values;
}

// Flag — increment/decrement of a parameter.
export function countDown(n: number) {
  while (n > 0) {
    n--;
  }
  return n;
}

// OK — computes a new value instead of writing back into the parameter.
export function withDiscountOk(price: number, rate: number) {
  const discounted = price - price * rate;
  return discounted;
}

// OK — returns a new object instead of mutating the parameter.
export function tagAsProcessedOk(item: { status: string }) {
  return { ...item, status: "processed" };
}

// OK — a same-named parameter on a nested function shadows the outer one;
// mutating the inner `price` must not flag against the outer parameter.
export function outer(price: number) {
  function inner(price: number) {
    price = price + 1; // flagged (inner's own parameter), not a false attribution to outer
    return price;
  }
  return inner(price);
}

// OK — reassigning a local variable, not a parameter.
export function localReassignOk(price: number) {
  let total = price;
  total = total * 2;
  return total;
}

// Flag — arrow function reassigning its own parameter.
export const withTax = (price: number, rate: number) => {
  price = price + price * rate;
  return price;
};

// Flag — compound assignment goes through the same AssignmentExpression node.
export function addTax(price: number, rate: number) {
  price += price * rate;
  return price;
}

// Flag — a closure mutating an outer function's parameter still flags,
// since the watch set is inherited by nested scopes.
export function makeIncrementer(total: number) {
  return () => {
    total = total + 1;
    return total;
  };
}
