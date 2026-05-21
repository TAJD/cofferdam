// Test-context unwrap. Must be silent: the engine dispatched the check
// (Rust extension), but the check's own ancestor walk recognises
// `#[cfg(test)] mod tests` and declines to emit.
#[cfg(test)]
mod tests {
    #[test]
    fn parses_one() {
        let n: i64 = "1".parse().unwrap();
        assert_eq!(n, 1);
    }
}
