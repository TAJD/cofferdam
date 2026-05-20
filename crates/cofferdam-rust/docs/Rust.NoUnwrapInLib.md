---
id: Rust.NoUnwrapInLib
category: Warning
base_priority: 12
default_severity: Medium
options: []
---

Calling `.unwrap()` or `.expect()` in library code panics on `None` or
`Err(_)`. Inside `main` or a test harness the panic is acceptable; in
library code paths the panic surfaces as an opaque crash to whoever is
calling the library.

The check fires on `.unwrap()` and `.expect(...)` call expressions that
are **not** inside a test context. A call counts as in test context when
any of its ancestors is:

* a function or item annotated with `#[cfg(test)]`,
* a function annotated with `#[test]`,
* or a module named `tests`.

Calls that hit those guards do not fire — assertions like
`parse(input).unwrap()` inside a `#[test]` are idiomatic and the test
harness handles the panic correctly.

## Example

```rust
// FIRES: lib code, no test guard.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()  // -> Rust.NoUnwrapInLib
}

// DOES NOT FIRE: enclosing function is #[test].
#[test]
fn round_trip() {
    assert_eq!(parse_id("42"), 42);
    let parsed: i64 = "1".parse().unwrap();  // ok
}

// DOES NOT FIRE: enclosing module is #[cfg(test)].
#[cfg(test)]
mod tests {
    use super::*;
    fn helper() -> i64 {
        "7".parse().unwrap()  // ok — inside cfg(test) module
    }
}
```

## What to do

* In library functions, return `Result<T, E>` and propagate via `?`.
* When the value is actually infallible (you've just checked
  `Option::is_some` two lines up), use `Option::expect` *with a
  descriptive message* — the message is the diagnostic when the
  invariant is later broken.
* Inside `main()`, prefer `?` and let the program exit with a
  formatted error rather than the default panic dump.

## Suppression

If a particular `unwrap()` is provably safe and the refactor isn't
worth it, narrow the suppression to the specific line:

```rust
// cofferdam-ignore: Rust.NoUnwrapInLib: provably safe — see invariant in comment above
let known_good = parse_input(SAFE_LITERAL).unwrap();
```

A blanket file-wide suppression masks new unwraps added later; prefer
inline directives.
