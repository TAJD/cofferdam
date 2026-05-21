//! Map cofferdam findings to LSP diagnostics.
//!
//! [`cofferdam_core::Issue`] uses 1-based `(line, column)` plus byte
//! offsets; [`lsp_types::Diagnostic`] uses 0-based `(line, character)`
//! with character counts that the spec defines as UTF-16 code units
//! (the legacy default; servers may negotiate UTF-8 via
//! [`PositionEncodingKind`] in `initialize` capabilities). cp5 picks
//! UTF-8 in its server capabilities — fewer surprises with
//! multi-byte source — but we publish 0-based line/character either
//! way.
//!
//! Why this lives in its own module: the conversion is the natural
//! seam where a test can pin "every cofferdam `Issue` translates to
//! a Diagnostic with the right code, severity, and range" without
//! booting an LSP server.

use cofferdam_core::{Issue, RelatedSpan, Severity, Span};
use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range,
    Url,
};

/// Convert one cofferdam [`Issue`] into an LSP diagnostic. `file_url`
/// is the URL the LSP will publish the diagnostic against — it must
/// match the URI the editor opened. Related spans become
/// [`DiagnosticRelatedInformation`] entries; cofferdam's cross-file
/// checks (duplicate exports, orphan exports) ship them so the
/// editor can resolve "where else" jumps natively.
pub fn issue_to_diagnostic(issue: &Issue, file_url: &Url) -> lsp_types::Diagnostic {
    let _ = file_url; // primary location uses issue.span; file_url is sent on the publish channel
    lsp_types::Diagnostic {
        range: span_to_range(&issue.span),
        severity: Some(severity_to_lsp(issue.severity)),
        code: Some(NumberOrString::String(issue.check_id.clone())),
        code_description: None,
        source: Some("cofferdam".to_string()),
        message: issue.message.clone(),
        related_information: related_to_lsp(&issue.related),
        tags: None,
        data: None,
    }
}

/// Translate a cofferdam [`Severity`] into the LSP four-level enum.
/// Cofferdam splits High and Critical; LSP collapses both to `Error`.
/// `Info` and `Low` map to `Information` and `Hint` so editors can
/// theme the underlines distinctly. `Medium` is the default
/// `Warning`.
pub fn severity_to_lsp(sev: Severity) -> DiagnosticSeverity {
    match sev {
        Severity::Critical => DiagnosticSeverity::ERROR,
        Severity::High => DiagnosticSeverity::ERROR,
        Severity::Medium => DiagnosticSeverity::WARNING,
        Severity::Low => DiagnosticSeverity::INFORMATION,
        Severity::Info => DiagnosticSeverity::HINT,
    }
}

/// Convert a cofferdam [`Span`] (1-based line/column) to an LSP
/// [`Range`] (0-based). The span carries both line/column and byte
/// offsets; LSP wants line/character. Character semantics depend on
/// the negotiated [`PositionEncodingKind`] — see [`crate::server`]
/// for the negotiated value.
///
/// When `end_byte` equals `start_byte`, the range collapses to a
/// zero-width point at the start. Editors render this as a caret
/// under the offending position, which matches cofferdam's intent
/// for findings without a meaningful "end" (most of them — the engine
/// doesn't reliably populate `end_byte` today; that's pinned to
/// cd-81a.2).
pub fn span_to_range(span: &Span) -> Range {
    let start_line = span.line.saturating_sub(1);
    let start_char = span.column.saturating_sub(1);
    let end_char = if span.end_byte > span.start_byte {
        // Approximate end column: span.column + (end_byte - start_byte).
        // Correct for ASCII; off for multibyte and multi-line spans.
        // The engine's span population is line-anchored in practice
        // — multi-line ranges are rare. Tightening this is cd-81a.2.
        let width = span.end_byte.saturating_sub(span.start_byte);
        start_char.saturating_add(width)
    } else {
        start_char
    };
    Range {
        start: Position {
            line: start_line,
            character: start_char,
        },
        end: Position {
            line: start_line,
            character: end_char,
        },
    }
}

fn related_to_lsp(related: &[RelatedSpan]) -> Option<Vec<DiagnosticRelatedInformation>> {
    if related.is_empty() {
        return None;
    }
    let infos = related
        .iter()
        .filter_map(|r| {
            // RelatedSpan carries a PathBuf; convert to Url for LSP.
            // Skip entries whose path can't be turned into a URL —
            // shouldn't happen in practice (paths come from disk
            // discovery), but defending here costs nothing.
            let url = Url::from_file_path(&r.file).ok()?;
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: url,
                    range: span_to_range(&r.span),
                },
                message: String::new(),
            })
        })
        .collect::<Vec<_>>();
    if infos.is_empty() {
        None
    } else {
        Some(infos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::Priority;
    use std::path::PathBuf;

    fn issue(sev: Severity, line: u32, col: u32) -> Issue {
        Issue {
            check_id: "Warning.TripleEquals".to_string(),
            message: "use === instead of ==".to_string(),
            file: PathBuf::from("a.ts"),
            span: Span {
                start_byte: 0,
                end_byte: 2,
                line,
                column: col,
            },
            priority: Priority::NORMAL,
            severity: sev,
            related: Vec::new(),
        }
    }

    #[test]
    fn severity_mapping_is_total() {
        assert_eq!(
            severity_to_lsp(Severity::Critical),
            DiagnosticSeverity::ERROR
        );
        assert_eq!(severity_to_lsp(Severity::High), DiagnosticSeverity::ERROR);
        assert_eq!(
            severity_to_lsp(Severity::Medium),
            DiagnosticSeverity::WARNING
        );
        assert_eq!(
            severity_to_lsp(Severity::Low),
            DiagnosticSeverity::INFORMATION
        );
        assert_eq!(severity_to_lsp(Severity::Info), DiagnosticSeverity::HINT);
    }

    #[test]
    fn span_one_one_becomes_zero_zero() {
        let range = span_to_range(&Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        });
        assert_eq!(range.start.line, 0);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.line, 0);
        assert_eq!(range.end.character, 0);
    }

    #[test]
    fn nonzero_width_span_extends_end_character() {
        let range = span_to_range(&Span {
            start_byte: 10,
            end_byte: 13,
            line: 5,
            column: 7,
        });
        // 1-based (5, 7) → 0-based (4, 6); width = 13 - 10 = 3 → end char 9.
        assert_eq!(range.start.line, 4);
        assert_eq!(range.start.character, 6);
        assert_eq!(range.end.line, 4);
        assert_eq!(range.end.character, 9);
    }

    #[test]
    fn issue_round_trips_to_diagnostic() {
        let url = Url::from_file_path("/tmp/a.ts").unwrap_or_else(|_| {
            // On Windows the absolute path needs a drive letter to make
            // a valid file URL. Use a Windows-style path in that case.
            Url::from_file_path("C:/tmp/a.ts").expect("valid file path")
        });
        let i = issue(Severity::Medium, 3, 5);
        let d = issue_to_diagnostic(&i, &url);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d.source.as_deref(), Some("cofferdam"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("Warning.TripleEquals".to_string()))
        );
        assert!(d.message.contains("==="));
        assert_eq!(d.range.start.line, 2);
        assert_eq!(d.range.start.character, 4);
        assert!(d.related_information.is_none());
    }

    #[test]
    fn related_spans_become_related_information() {
        // Synthesise a related span; on Windows use a drive-prefixed path
        // so `Url::from_file_path` succeeds.
        let related_path = if cfg!(windows) {
            PathBuf::from("C:/tmp/other.ts")
        } else {
            PathBuf::from("/tmp/other.ts")
        };
        let mut i = issue(Severity::Medium, 1, 1);
        i.related.push(RelatedSpan {
            file: related_path.clone(),
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 2,
                column: 3,
            },
        });
        let url = Url::from_file_path(if cfg!(windows) {
            "C:/tmp/a.ts"
        } else {
            "/tmp/a.ts"
        })
        .expect("valid file URL");
        let d = issue_to_diagnostic(&i, &url);
        let related = d
            .related_information
            .as_ref()
            .expect("related info populated");
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.range.start.line, 1);
        assert_eq!(related[0].location.range.start.character, 2);
    }
}
