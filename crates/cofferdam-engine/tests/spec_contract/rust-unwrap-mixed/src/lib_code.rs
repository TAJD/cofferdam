// Lib-context unwrap. Engine must route this file (.rs extension →
// Language::Rust) to Rust.NoUnwrapInLib via per-language dispatch. The
// finding lands on line 4.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}
