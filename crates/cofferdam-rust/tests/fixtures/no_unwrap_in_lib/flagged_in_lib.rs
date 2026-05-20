// Library code with no test guards — every .unwrap() / .expect() here
// should fire Rust.NoUnwrapInLib. Used as a positive fixture.

pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}

pub fn first_char(s: &str) -> char {
    s.chars().next().expect("non-empty string")
}

pub fn lookup(map: &std::collections::HashMap<String, String>, key: &str) -> String {
    map.get(key).unwrap().clone()
}
