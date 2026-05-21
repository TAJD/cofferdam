---
id: Rust.NoUnwrapInLib
category: Warning
base_priority: 12
default_severity: Medium
options: []
---

Calling `.unwrap()` in library code panics on `None` or `Err(_)` with
**no diagnostic context** — the caller gets a generic backtrace and
nowhere to start debugging. The Rust convention is to use
`.expect("<reason>")` instead when the value is provably infallible:
the message describes the invariant, so when it breaks the diagnostic
tells the next reader what assumption failed.

This check therefore flags **only `.unwrap()`** — bare, no-context
panics. `.expect("<message>")` with a descriptive message is the
documented escape valve and is **not** flagged. The check matches
clippy's `unwrap_used` (which flags `.unwrap()`) rather than its
`expect_used` (which flags `.expect()` and is rarely enabled).

The check fires on `.unwrap()` call expressions that are **not**
inside a test context. A call counts as in test context when any of
its ancestors is:

* a function or item annotated with `#[cfg(test)]`,
* a function annotated with `#[test]`,
* or a module named `tests`.

Calls that hit those guards do not fire — assertions like
`parse(input).unwrap()` inside a `#[test]` are idiomatic and the test
harness handles the panic correctly.

## Example

```rust
// FIRES: bare .unwrap() in lib code, no context.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()  // -> Rust.NoUnwrapInLib
}

// DOES NOT FIRE: descriptive .expect() carries the proof of safety.
pub fn first_char(s: &str) -> char {
    s.chars().next().expect("caller guarantees non-empty input")
}

// DOES NOT FIRE: enclosing function is #[test].
#[test]
fn round_trip() {
    assert_eq!(parse_id("42"), 42);
    let parsed: i64 = "1".parse().unwrap();  // ok in tests
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
  `Option::is_some` two lines up, you hold a `Mutex::lock()` and a
  poisoned lock is unrecoverable, you've serialized an internal type
  that can't fail), use `.expect("<reason>")` — the message names
  the invariant for the next reader when it breaks. This is **not
  flagged**.
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
