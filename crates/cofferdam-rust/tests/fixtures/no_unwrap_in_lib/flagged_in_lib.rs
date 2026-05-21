// Library code with no test guards. Every bare `.unwrap()` here should
// fire Rust.NoUnwrapInLib. `.expect("<message>")` is documented
// alternative for proven-safe cases and is NOT flagged.

pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}

pub fn first_char(s: &str) -> char {
    // Allow-listed: descriptive `.expect()` carries the proof.
    s.chars().next().expect("non-empty string")
}

pub fn lookup(map: &std::collections::HashMap<String, String>, key: &str) -> String {
    map.get(key).unwrap().clone()
}
