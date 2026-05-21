// Lib-context unwrap. Engine must route this file (.rs extension →
// Language::Rust) to Rust.NoUnwrapInLib via per-language dispatch.
// The doc comment below silences Rust.MissingPubDoc so this fixture
// stays focused on dispatch — the unwrap finding is the contract.
/// Parse `s` as an `i64`. Panics on a non-integer input — used here
/// to trigger Rust.NoUnwrapInLib.
pub fn parse_id(s: &str) -> i64 {
    s.parse::<i64>().unwrap()
}
