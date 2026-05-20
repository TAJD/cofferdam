// All unwraps live inside `#[cfg(test)] mod tests` — should NOT fire.

pub fn parse_id(s: &str) -> Option<i64> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> i64 {
        "7".parse().unwrap()
    }

    fn deeper() {
        let value = helper();
        let s = format!("{value}");
        let back: i64 = s.parse().expect("round-trip");
        assert_eq!(back, value);
    }
}
