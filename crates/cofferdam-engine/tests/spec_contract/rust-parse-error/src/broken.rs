// Deliberately malformed Rust: missing function name. tree-sitter-rust
// recovers with an ERROR node; the engine surfaces that as
// Warning.ParseError via the cd-0039 dispatch path.
fn () {
    println!("oops");
}
