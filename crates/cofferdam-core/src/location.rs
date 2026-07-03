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
            range: LocationRange::Bytes {
                start,
                end,
                line,
                column,
            },
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

    /// Byte offset where the range starts, or 0 for ranges with no byte
    /// representation (`LineCol` / `Custom`). TS findings are always
    /// `Bytes`, so this is exact for the only adapter shipping today.
    pub fn start_byte(&self) -> u32 {
        match &self.range {
            LocationRange::Bytes { start, .. } => *start,
            _ => 0,
        }
    }

    /// Byte offset where the range ends, or 0 for non-byte ranges.
    pub fn end_byte(&self) -> u32 {
        match &self.range {
            LocationRange::Bytes { end, .. } => *end,
            _ => 0,
        }
    }

    /// 1-based start line. `Bytes` and `LineCol` carry it; `Custom` has no
    /// line concept and returns 0.
    pub fn line(&self) -> u32 {
        match &self.range {
            LocationRange::Bytes { line, .. } => *line,
            LocationRange::LineCol { start_line, .. } => *start_line,
            LocationRange::Custom { .. } => 0,
        }
    }

    /// 1-based start column. `Bytes` and `LineCol` carry it; `Custom`
    /// returns 0.
    pub fn column(&self) -> u32 {
        match &self.range {
            LocationRange::Bytes { column, .. } => *column,
            LocationRange::LineCol { start_col, .. } => *start_col,
            LocationRange::Custom { .. } => 0,
        }
    }

    /// `Some((start, end))` byte offsets for `Bytes` ranges, `None`
    /// otherwise. Formatters use this instead of `start_byte()`/
    /// `end_byte()` when they need to distinguish "no byte data" from
    /// "byte data happens to be 0".
    pub fn byte_range(&self) -> Option<(u32, u32)> {
        match &self.range {
            LocationRange::Bytes { start, end, .. } => Some((*start, *end)),
            _ => None,
        }
    }

    /// `Some((end_line, end_col))` for `LineCol` ranges, `None` otherwise
    /// (`Bytes` has no separate end line/column; `Custom` has none at
    /// all).
    pub fn end_line_col(&self) -> Option<(u32, u32)> {
        match &self.range {
            LocationRange::LineCol {
                end_line, end_col, ..
            } => Some((*end_line, *end_col)),
            _ => None,
        }
    }
}

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

    #[test]
    fn bytes_variant_round_trips_through_json() {
        let r = LocationRange::Bytes {
            start: 10,
            end: 20,
            line: 2,
            column: 5,
        };
        let json = serde_json::to_string(&r).unwrap();
        // Tagged by `kind`; field names are the schema contract.
        assert!(json.contains("\"kind\":\"bytes\""), "got: {json}");
        let back: LocationRange = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn linecol_variant_round_trips_through_json() {
        let r = LocationRange::LineCol {
            start_line: 2,
            start_col: 5,
            end_line: 4,
            end_col: 1,
        };
        let back: LocationRange =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn custom_variant_round_trips_through_json() {
        let r = LocationRange::Custom {
            ns: SmolStr::new("sql"),
            id: SmolStr::new("stmt:3"),
        };
        let back: LocationRange =
            serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn location_round_trips_through_json() {
        let loc = Location::bytes(Uri::from_path(Path::new("a.ts")), 0, 4, 1, 1);
        let back: Location = serde_json::from_str(&serde_json::to_string(&loc).unwrap()).unwrap();
        assert_eq!(loc, back);
    }

    #[test]
    fn from_span_bridges_path_plus_span() {
        let span = Span {
            start_byte: 3,
            end_byte: 7,
            line: 1,
            column: 4,
        };
        let loc = Location::from_span(Path::new("b.ts"), span);
        assert_eq!(loc.uri.as_str(), "file://b.ts");
        assert_eq!(
            loc.range,
            LocationRange::Bytes {
                start: 3,
                end: 7,
                line: 1,
                column: 4
            }
        );
    }

    #[test]
    fn accessors_extract_bytes_variant_fields() {
        let loc = Location::bytes(Uri::from_path(Path::new("a.ts")), 3, 7, 2, 5);
        assert_eq!(loc.start_byte(), 3);
        assert_eq!(loc.end_byte(), 7);
        assert_eq!(loc.line(), 2);
        assert_eq!(loc.column(), 5);
    }

    #[test]
    fn accessors_degrade_for_non_byte_variants() {
        let custom = Location {
            uri: Uri::new("sql://migrations"),
            range: LocationRange::Custom {
                ns: SmolStr::new("sql"),
                id: SmolStr::new("stmt:3"),
            },
        };
        assert_eq!(custom.start_byte(), 0);
        assert_eq!(custom.end_byte(), 0);
        assert_eq!(custom.line(), 0);
        assert_eq!(custom.column(), 0);

        let linecol = Location {
            uri: Uri::new("gen://out.ts"),
            range: LocationRange::LineCol {
                start_line: 4,
                start_col: 2,
                end_line: 4,
                end_col: 9,
            },
        };
        assert_eq!(linecol.line(), 4);
        assert_eq!(linecol.column(), 2);
        assert_eq!(linecol.start_byte(), 0);
    }

    #[test]
    fn location_is_hashable_for_cache_keys() {
        use std::collections::HashSet;
        let a = Location::from_span(
            Path::new("a.ts"),
            Span {
                start_byte: 0,
                end_byte: 1,
                line: 1,
                column: 1,
            },
        );
        let b = a.clone();
        let mut set = HashSet::new();
        set.insert(a);
        assert!(
            set.contains(&b),
            "Location must be usable as a cache/baseline key"
        );
    }
}
