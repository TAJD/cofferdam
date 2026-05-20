// Unwraps inside `#[test]` functions (not in a tests module) — should NOT fire.

pub fn parse_id(s: &str) -> Option<i64> {
    s.parse().ok()
}

#[test]
fn round_trip() {
    let value: i64 = "42".parse().unwrap();
    assert_eq!(value, 42);
}

#[test]
fn another_test() {
    "1".parse::<i64>().expect("parse succeeds");
}
