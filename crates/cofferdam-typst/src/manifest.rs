//! `typst.toml` manifest parsing — values plus per-key byte spans.
//!
//! Two parses of the same text, deliberately: `toml` (serde) gives typed
//! `Option<...>` values so "missing" is `None`; `toml_edit` gives byte
//! spans for the same keys so findings point at the offending line
//! instead of line 1. Neither crate alone does both.

use cofferdam_core::{span_from_bytes, Span};
use serde::Deserialize;

/// Every Universe-relevant `[package]` field. `None` means "absent from
/// the manifest" — checks read that directly rather than distinguishing
/// "empty string" from "missing".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Manifest {
    pub name: Option<String>,
    pub version: Option<String>,
    pub entrypoint: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub description: Option<String>,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawDocument {
    package: Option<Manifest>,
}

/// Parse `typst.toml` text into typed values. Returns `Err` only on
/// malformed TOML — a well-formed document with no `[package]` table
/// yields `Manifest::default()` (every field `None`), which
/// `Typst.ManifestRequiredFields` then flags.
pub fn parse_manifest(text: &str) -> Result<Manifest, toml::de::Error> {
    let doc: RawDocument = toml::from_str(text)?;
    Ok(doc.package.unwrap_or_default())
}

/// Byte/line/col span for each `[package]` key present in the manifest.
/// `None` for keys that are absent (nothing to point at) or when the
/// document failed to parse as `toml_edit` (defensive — `parse_manifest`
/// would already have rejected genuinely malformed TOML).
#[derive(Debug, Clone, Default)]
pub struct ManifestSpans {
    pub name: Option<Span>,
    pub version: Option<Span>,
    pub entrypoint: Option<Span>,
    pub authors: Option<Span>,
    pub license: Option<Span>,
    pub description: Option<Span>,
    pub exclude: Option<Span>,
}

/// Extract per-key spans from the same manifest text. Best-effort: a
/// `toml_edit` parse failure (shouldn't happen if `parse_manifest`
/// already succeeded on the same text) degrades to all-`None` rather
/// than propagating an error, since spans are a "nice to have" for
/// finding locations, not required for correctness.
///
/// Parses via `ImDocument` rather than `DocumentMut` — `DocumentMut`
/// (the mutation-oriented API) discards byte spans on parse;
/// `ImDocument` (the immutable/read-only API) retains them.
pub fn parse_manifest_spans(text: &str) -> ManifestSpans {
    let Ok(doc) = text.parse::<toml_edit::ImDocument<String>>() else {
        return ManifestSpans::default();
    };
    let Some(table) = doc.get("package").and_then(toml_edit::Item::as_table) else {
        return ManifestSpans::default();
    };
    ManifestSpans {
        name: key_span(table, "name", text),
        version: key_span(table, "version", text),
        entrypoint: key_span(table, "entrypoint", text),
        authors: key_span(table, "authors", text),
        license: key_span(table, "license", text),
        description: key_span(table, "description", text),
        exclude: key_span(table, "exclude", text),
    }
}

fn key_span(table: &toml_edit::Table, key: &str, text: &str) -> Option<Span> {
    let k = table.key(key)?;
    let key_range = k.span()?;
    let item = table.get(key)?;
    let end = item.span().map(|r| r.end).unwrap_or(key_range.end);
    Some(span_from_bytes(text, key_range.start as u32, end as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_present_fields() {
        let text = r#"
[package]
name = "my-pkg"
version = "1.0.0"
"#;
        let m = parse_manifest(text).unwrap();
        assert_eq!(m.name.as_deref(), Some("my-pkg"));
        assert_eq!(m.version.as_deref(), Some("1.0.0"));
        assert_eq!(m.entrypoint, None);
    }

    #[test]
    fn missing_package_table_yields_all_none() {
        let m = parse_manifest("").unwrap();
        assert!(m.name.is_none());
        assert!(m.license.is_none());
    }

    #[test]
    fn spans_point_at_key_line_not_line_one() {
        let text = "\n\n[package]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n";
        let spans = parse_manifest_spans(text);
        let name_span = spans.name.expect("name key present");
        assert_eq!(name_span.line, 4);
        let version_span = spans.version.expect("version key present");
        assert_eq!(version_span.line, 5);
    }

    #[test]
    fn spans_absent_for_missing_keys() {
        let text = "[package]\nname = \"x\"\n";
        let spans = parse_manifest_spans(text);
        assert!(spans.name.is_some());
        assert!(spans.license.is_none());
    }
}
