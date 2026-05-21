// Mixed file: 1 lib `.unwrap()` (fires), 1 lib `.expect()` (allow-listed),
// 1 test `.unwrap()` (silent via cfg_test), 1 test `.expect()` (silent).
// The lib fire is at line 5 — the assertion in the check tests pins it.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}

pub fn first_char(s: &str) -> char {
    // `.expect(...)` with a non-empty message is the documented
    // escape valve for proven-safe panics; NOT flagged.
    s.chars().next().expect("non-empty string")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_42() {
        assert_eq!(parse_id("42"), 42);
        let inner: i64 = "1".parse().unwrap();
        assert_eq!(inner, 1);
    }

    #[test]
    fn first_char_works() {
        assert_eq!(first_char("hello"), 'h');
        let _ = "abc".chars().next().expect("non-empty");
    }
}
