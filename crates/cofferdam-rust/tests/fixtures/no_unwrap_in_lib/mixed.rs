// Mixed file: 2 lib unwraps (should fire), 2 test unwraps (should not).
// Lib lines are at 4 and 9; the assertion in the check tests pins those.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}

pub fn first_char(s: &str) -> char {
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
