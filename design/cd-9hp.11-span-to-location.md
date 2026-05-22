# Span → Location Refactor — Implementation Plan (cd-9hp.11)

> **For agentic workers:** Steps use checkbox (`- [ ]`) syntax for tracking. Implement task-by-task with `cargo test -p cofferdam-core` after each.

**Goal:** Introduce a `Location` type that generalizes `Span` to non-text artifacts (packages, generated code, opaque adapter ranges), then migrate every public-facing `Span` consumer onto it without regressing the TS path.

**Architecture:** `Location { uri: Uri, range: LocationRange }` lands additively in `cofferdam-core` (PR 1, nothing consumes it). Then `Issue`/`RelatedSpan` gain `location` (PR 2), formatters + suppression render every variant (PR 3), and the plugin SDK re-exports it (PR 4). Byte-range findings keep precomputed line/column so formatters never need source text back.

**Tech Stack:** Rust, serde (+serde_json for round-trip tests), smol_str (interned identifiers, already a workspace dep).

---

## Scope & Sequencing

The user chose the **full migration** (all 4 PRs). They ship as four independent, additive PRs (the bead's checkpoint plan). **This document fully details PR 1 only** — PRs 2–4 are scoped at task level here and get their own bite-sized plans once PR 1's type shape is final and each consumer has been read. Rationale: PR 2–4 code depends on PR 1's locked type and on reading ~30 consumers; writing it now would be speculative placeholder code, which this skill forbids.

## Design decisions (refinements to the bead's sketch — REVIEW THESE)

The bead (`cd-9hp.11` DESCRIPTION) sketched the type two weeks before the code was re-read. Four corrections, all within implementation-design scope:

1. **`LocationRange::Bytes` retains `line` + `column`.** The bead's `Bytes { start, end }` dropped them. But today's `Span` precomputes line/col precisely so formatters (`text.rs` prints `file:line:col`) don't need the source text back at render time. Dropping them would force every formatter to reacquire source. Keep them: `Bytes { start, end, line, column }`.
2. **No bare `From<Span> for Location`.** `Span` carries no path; a blanket `From` would fabricate an empty/garbage `uri`. The honest bridge is `Location::from_span(path, span)` — the path comes from `Issue::file` / `RelatedSpan::file` at the call site.
3. **`LineCol` uses named fields, not tuples.** The bead's `start: (u32, u32)` serializes to JSON arrays (`[12, 5]`). Named fields (`start_line`, `start_col`, …) give a stable, self-describing JSON schema. The schema is the contract (CLAUDE.md: "JSON schema is the contract — additive changes only").
4. **`Uri` is a `SmolStr` newtype** with scheme-aware constructors (`from_path` → `file://`). Keeps `cofferdam-core` dependency-light (smol_str is tiny and already pinned) while giving non-TS adapters `package://` / `gen://` later.

`Location` derives `PartialEq, Eq, Hash` — **load-bearing**: the three engine caches (`run_cache`, `disk_cache`, `findings_cache`) and `baseline.rs` rule-signature keying rely on `Span: Hash + Eq` today. `Location` is **not** `Copy` (it owns a `SmolStr`); PR 2 absorbs the `Copy → Clone` fallout at call sites.

---

## File Structure (PR 1)

- Create: `crates/cofferdam-core/src/location.rs` — `Uri`, `LocationRange`, `Location`, constructors, `from_span` bridge, unit tests.
- Modify: `crates/cofferdam-core/src/lib.rs` — add `pub mod location;` + re-export `Location, LocationRange, Uri`.
- Modify: `crates/cofferdam-core/Cargo.toml` — add `smol_str` to `[dependencies]`, `serde_json` to `[dev-dependencies]`.

PR 1 touches no consumer. `Span`, `Issue`, `RelatedSpan` are unchanged.

---

## PR 1 — Location type lands (additive, no consumers)

### Task 1: Add dependencies

**Files:**
- Modify: `crates/cofferdam-core/Cargo.toml`

- [ ] **Step 1: Add `smol_str` to `[dependencies]`**

In `crates/cofferdam-core/Cargo.toml`, under `[dependencies]` (alphabetical-ish, after `serde`):

```toml
serde = { workspace = true }
smol_str = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Add `serde_json` to `[dev-dependencies]`**

```toml
[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
tempfile = "3"
```

- [ ] **Step 3: Verify the workspace still resolves**

Run: `cargo build -p cofferdam-core`
Expected: compiles clean (no new code yet; just deps available).

- [ ] **Step 4: Commit**

```bash
git add crates/cofferdam-core/Cargo.toml
git commit -m "build(core): add smol_str dep + serde_json dev-dep for Location (cd-9hp.11)"
```

### Task 2: Define `Uri`

**Files:**
- Create: `crates/cofferdam-core/src/location.rs`
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Create `crates/cofferdam-core/src/location.rs` with only the test (it won't compile yet — that is the "fail"):

```rust
//! `Location` — the artifact-agnostic generalization of `Span` (cd-9hp.11).
//!
//! `Span` (`crate::issue::Span`) is byte-offset + line/column, which assumes
//! a text source on disk. `Location` pairs a `Uri` (which resource) with a
//! `LocationRange` (where within it), so non-text adapters — packages,
//! generated code, opaque statement indices — can carry findings through
//! suppression, baselines, and formatters the same way TS findings do.
//!
//! PR 1 (this file) is purely additive: nothing in the engine constructs a
//! `Location` yet. `Issue`/`RelatedSpan` migrate in a follow-up.

use crate::issue::Span;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_from_path_uses_file_scheme() {
        let uri = Uri::from_path(Path::new("src/main.ts"));
        assert_eq!(uri.as_str(), "file://src/main.ts");
    }

    #[test]
    fn uri_new_preserves_arbitrary_scheme() {
        let uri = Uri::new("package://calendaring@0.1.0");
        assert_eq!(uri.as_str(), "package://calendaring@0.1.0");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cofferdam-core --lib location`
Expected: FAIL — `cannot find type Uri in this scope`.

- [ ] **Step 3: Implement `Uri`**

Insert above the `#[cfg(test)]` block:

```rust
/// Resource identifier for a finding. A scheme-prefixed string:
/// `file://<path>` for text source on disk (every TS finding), or
/// `package://<name>@<ver>` / `gen://<...>` for adapters whose artifact
/// is not a path. Interned via `SmolStr` — finding volume is high and
/// most URIs repeat across a file's findings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Uri(SmolStr);

impl Uri {
    /// Wrap an already-formed URI string (caller supplies the scheme).
    pub fn new(s: impl Into<SmolStr>) -> Uri {
        Uri(s.into())
    }

    /// Build a `file://` URI from a path. Uses the lossy display form —
    /// cofferdam paths are workspace-relative display paths, not OS
    /// handles, so this round-trips for human + editor consumption.
    pub fn from_path(path: &Path) -> Uri {
        Uri(SmolStr::new(format!("file://{}", path.display())))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cofferdam-core --lib location`
Expected: PASS (2 tests). `Location`/`LocationRange` not referenced yet.

- [ ] **Step 5: Wire the module into the crate**

In `crates/cofferdam-core/src/lib.rs`, add after `pub mod lines;` (keep alphabetical):

```rust
pub mod location;
```

And add the re-export after the `issue::{...}` line (line ~38):

```rust
pub use location::{Location, LocationRange, Uri};
```

This will not compile yet (`Location`/`LocationRange` undefined) — Task 3 fixes it. If you want a green tree between tasks, defer this step's re-export edit until Task 4 Step 1. (The `pub mod location;` line alone is fine now.)

- [ ] **Step 6: Commit**

```bash
git add crates/cofferdam-core/src/location.rs crates/cofferdam-core/src/lib.rs
git commit -m "feat(core): add Uri newtype for Location (cd-9hp.11)"
```

### Task 3: Define `LocationRange`

**Files:**
- Modify: `crates/cofferdam-core/src/location.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
    #[test]
    fn bytes_variant_round_trips_through_json() {
        let r = LocationRange::Bytes { start: 10, end: 20, line: 2, column: 5 };
        let json = serde_json::to_string(&r).unwrap();
        // Tagged by `kind`; field names are the schema contract.
        assert!(json.contains("\"kind\":\"bytes\""), "got: {json}");
        let back: LocationRange = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn linecol_variant_round_trips_through_json() {
        let r = LocationRange::LineCol { start_line: 2, start_col: 5, end_line: 4, end_col: 1 };
        let back: LocationRange = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn custom_variant_round_trips_through_json() {
        let r = LocationRange::Custom { ns: SmolStr::new("sql"), id: SmolStr::new("stmt:3") };
        let back: LocationRange = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cofferdam-core --lib location`
Expected: FAIL — `cannot find type LocationRange`.

- [ ] **Step 3: Implement `LocationRange`**

Insert after the `Uri` impl block:

```rust
/// Where within a resource a finding sits. Three representations cover the
/// known artifact classes; adapters that fit none use `Custom`.
///
/// Serialized as an internally-tagged union (`{"kind": "...", ...}`) so the
/// JSON schema is self-describing and additive — a new variant cannot break
/// a consumer that switches on `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocationRange {
    /// Byte offsets plus precomputed 1-based line/column — the current
    /// `Span` shape. line/col are kept (not dropped) so formatters render
    /// `file:line:col` without re-reading the source.
    Bytes {
        start: u32,
        end: u32,
        line: u32,
        column: u32,
    },
    /// 1-based line/column pairs only. For sources where lines are stable
    /// but byte offsets are awkward (post-formatter output, generated code).
    LineCol {
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
    },
    /// Adapter-defined opaque range: a namespace tag plus an opaque id.
    /// Example: a SQL migration adapter identifies a statement by index,
    /// emitting `{ ns: "sql", id: "stmt:3" }`. Cofferdam never parses `id`;
    /// the adapter owns its meaning.
    Custom { ns: SmolStr, id: SmolStr },
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p cofferdam-core --lib location`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/cofferdam-core/src/location.rs
git commit -m "feat(core): add LocationRange tagged union (cd-9hp.11)"
```

### Task 4: Define `Location` + the `Span` bridge

**Files:**
- Modify: `crates/cofferdam-core/src/location.rs`
- Modify: `crates/cofferdam-core/src/lib.rs` (re-export, if deferred from Task 2)

- [ ] **Step 1: Write the failing test**

Add to `mod tests`:

```rust
    #[test]
    fn location_round_trips_through_json() {
        let loc = Location::bytes(Uri::from_path(Path::new("a.ts")), 0, 4, 1, 1);
        let back: Location = serde_json::from_str(&serde_json::to_string(&loc).unwrap()).unwrap();
        assert_eq!(loc, back);
    }

    #[test]
    fn from_span_bridges_path_plus_span() {
        let span = Span { start_byte: 3, end_byte: 7, line: 1, column: 4 };
        let loc = Location::from_span(Path::new("b.ts"), span);
        assert_eq!(loc.uri.as_str(), "file://b.ts");
        assert_eq!(
            loc.range,
            LocationRange::Bytes { start: 3, end: 7, line: 1, column: 4 }
        );
    }

    #[test]
    fn location_is_hashable_for_cache_keys() {
        use std::collections::HashSet;
        let a = Location::from_span(Path::new("a.ts"), Span { start_byte: 0, end_byte: 1, line: 1, column: 1 });
        let b = a.clone();
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b), "Location must be usable as a cache/baseline key");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cofferdam-core --lib location`
Expected: FAIL — `cannot find type Location` / `no function bytes`.

- [ ] **Step 3: Implement `Location`**

Insert after the `LocationRange` enum:

```rust
/// A finding's location: which resource (`uri`) and where within it
/// (`range`). Generalizes `Span`. Not `Copy` — it owns a `SmolStr` — so
/// migrated call sites clone where they used to copy. `Hash + Eq` is
/// load-bearing: the engine's finding caches and baseline rule-signature
/// keying use it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Location {
    pub uri: Uri,
    pub range: LocationRange,
}

impl Location {
    /// Byte-range location with precomputed line/column — the common TS
    /// path. Mirrors a `Span` plus its owning `Uri`.
    pub fn bytes(uri: Uri, start: u32, end: u32, line: u32, column: u32) -> Location {
        Location {
            uri,
            range: LocationRange::Bytes { start, end, line, column },
        }
    }

    /// Bridge from the legacy `Span` + the path it was found in. This is
    /// the constructor migrated built-in checks call (`Location::from_span(
    /// &issue.file, span)`); there is deliberately no bare `From<Span>`
    /// because a `Span` alone has no resource identity.
    pub fn from_span(path: &Path, span: Span) -> Location {
        Location::bytes(
            Uri::from_path(path),
            span.start_byte,
            span.end_byte,
            span.line,
            span.column,
        )
    }
}
```

- [ ] **Step 4: Ensure the re-export is in place**

Confirm `crates/cofferdam-core/src/lib.rs` has both lines (add if deferred from Task 2):

```rust
pub mod location;
// ...
pub use location::{Location, LocationRange, Uri};
```

- [ ] **Step 5: Run the full core test suite**

Run: `cargo test -p cofferdam-core`
Expected: PASS — the 3 new `Location` tests plus all pre-existing core tests.

- [ ] **Step 6: Verify the whole workspace still builds and lints**

Run:
```bash
cargo build --workspace
cargo clippy -p cofferdam-core --all-targets -- -D warnings
cargo fmt --all -- --check
```
Expected: all clean. (No consumer touched, so the workspace is unaffected.)

- [ ] **Step 7: Commit**

```bash
git add crates/cofferdam-core/src/location.rs crates/cofferdam-core/src/lib.rs
git commit -m "feat(core): add Location type + Span bridge (cd-9hp.11 PR1)"
```

---

## PRs 2–4 — Roadmap (detailed plans written as each predecessor lands)

These are intentionally **not** bite-sized yet (see Scope note). Each becomes its own plan doc after the prior PR merges.

### PR 2 — `Issue` + `RelatedSpan` carry `Location`
- Add `location: Location` to `Issue` and `RelatedSpan`; decide the `Issue.file` question: **keep `file: PathBuf` alongside `location`** (least disruptive — baseline + JSON keep their current `file` field; `location.uri` is derived) vs. subsume `file` into `location.uri` (cleaner, but a breaking JSON-schema change requiring a cd-9hp.12 version bump). **Recommendation: keep `file` for PR 2; revisit subsumption when a non-`file://` adapter actually ships.**
- Transitional dual field: keep `span: Span` so existing checks compile; populate `location` via `Location::from_span(&file, span)`. Built-in checks migrate to constructing `location` directly.
- `Copy → Clone` fallout at every `Issue`/`RelatedSpan` construction site (~30 across `cofferdam-checks`, `cofferdam-rust`, engine).
- Consumers to read first: `cofferdam-engine/src/baseline.rs` (12 refs — heaviest), `run_cache.rs`, `disk_cache.rs`, `findings_cache.rs`.
- Verify: full `cargo test --workspace` + real-repo (`bestefforttools`, `gistreact`) — finding output byte-identical.

### PR 3 — Formatters + suppression render every variant
- **Four** formatters, not three: `text.rs`, `json.rs`, `sarif.rs`, **`compact.rs`** (bead missed compact). Each must handle `Bytes` / `LineCol` / `Custom`.
- text: `file.ts:12:5` (Bytes/LineCol) vs `migration_042.sql[stmt 3]` (Custom).
- json: preserve the **actual** current field names (`span: {start_byte, end_byte, line, column}`) via a compat shim for `Bytes`; add `range` tagged union additively. Bump schema per cd-9hp.12 policy.
- sarif: Bytes/LineCol map natively to `region`; `Custom` → `logicalLocations` + `properties` bag. Document degradation.
- Also a consumer the bead missed: `cofferdam-lsp/src/diagnostic.rs` (Span → LSP Diagnostic) — must handle non-Bytes gracefully (LSP needs line/col; `Custom` has none → skip or 0:0 with a note).
- suppression target syntax: `LocationRange`-aware (coordinate with cd-T6 adapter identity); only the byte path is exercised today.

### PR 4 — Plugin SDK
- `packages/check-sdk/` re-exports `Location`; existing plugins compile unchanged via a `Span`-wrapping compat shim.
- Smallest PR (~half day). Verify against any example plugin under `examples-plugins/` if present.

---

## Self-Review

**Spec coverage (PR 1 acceptance items):**
- ✅ "`Location` and `LocationRange` types in `cofferdam-core`; round-trip serialisation tests" → Tasks 2–4, JSON round-trip per variant + full `Location`.
- ✅ "`Location` constructible from `Span`" → `Location::from_span(path, span)` (Task 4) — with the documented divergence (path required).
- ⏭️ Issue/formatters/SDK/suppression acceptance items → PRs 2–4 (roadmap).
- ✅ "No regression on existing fixture suite" → Task 4 Step 6 (`cargo build --workspace`, clippy, fmt); PR 1 touches no consumer so no fixture can shift.

**Placeholder scan:** none — every code step has complete code; every run step has an exact command + expected result.

**Type consistency:** `Uri::new`/`from_path`/`as_str`, `LocationRange::{Bytes,LineCol,Custom}` field names, `Location::bytes`/`from_span` are used identically across tasks and the PR 2–4 roadmap. `Bytes` carries `line`+`column` everywhere. `LineCol` uses `start_line/start_col/end_line/end_col` everywhere.

**Divergences from bead flagged for user review:** (1) Bytes keeps line/col; (2) `from_span(path, span)` not bare `From<Span>`; (3) LineCol named fields not tuples; (4) Uri = SmolStr newtype. See "Design decisions" section.
