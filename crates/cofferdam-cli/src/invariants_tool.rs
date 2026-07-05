//! Library-only helper backing `cofferdam-mcp`'s `cofferdam.invariants`
//! tool — surfaces the parsed `cofferdam.invariants.toml` (layers,
//! public_api, boundaries, invariants) as JSON. No CLI subcommand
//! wraps this; the CLI has no equivalent today (CD-60).

use std::path::Path;

use cofferdam_core::invariants::InvariantsSpec;

/// Discover and load `cofferdam.invariants.toml` starting from `start`.
/// Returns `Ok(None)` when no invariants file is present (not an error —
/// most repos don't have one), `Err` on a malformed file.
pub fn load_invariants(start: &Path) -> Result<Option<InvariantsSpec>, String> {
    let Some(path) = cofferdam_core::invariants::discover(start) else {
        return Ok(None);
    };
    cofferdam_core::invariants::load(&path)
        .map(Some)
        .map_err(|e| e.to_string())
}
