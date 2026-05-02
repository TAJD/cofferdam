//! `cofferdam-engine` — orchestration.
//!
//! Phase 1: parse each file with oxc, expose the AST through CheckContext,
//! emit Warning.ParseError on fatal parse failures. Still single-threaded;
//! rayon comes in a follow-up once the AST seam is stable.

pub mod discover;

pub use discover::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};

use std::path::{Path, PathBuf};

use cofferdam_core::parser::{parse_fatal, parse_into, ParsedView};
use cofferdam_core::{Allocator, Check, CheckContext, Issue, Priority, Severity, SourceFile, Span};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Engine {
    checks: Vec<Box<dyn Check>>,
}

impl Engine {
    pub fn new(checks: Vec<Box<dyn Check>>) -> Self {
        Self { checks }
    }

    /// Run all configured checks against every file in `paths`.
    ///
    /// For each file: read text, parse with oxc, build CheckContext with
    /// the parsed view, run every check. Fatal parse failures emit a
    /// `Warning.ParseError` issue and the per-file check loop is skipped
    /// — checks that need the AST would have nothing to operate on.
    /// Non-fatal parse errors still produce a usable AST; checks run and
    /// the diagnostics are exposed via `ParsedView.diagnostics`.
    pub fn analyze<P: AsRef<Path>>(&self, paths: &[P]) -> Result<Vec<Issue>, EngineError> {
        let mut issues = Vec::new();

        for path in paths {
            let path = path.as_ref();
            let text = std::fs::read_to_string(path).map_err(|source| EngineError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
            let file = SourceFile::new(path.to_path_buf(), text);

            // Per-file allocator. Lives until the file's checks finish,
            // then drops with the AST it owns. Bumpalo allocation makes
            // this trivially cheap.
            let allocator = Allocator::default();
            let parsed_return = parse_into(&allocator, &file);

            if parse_fatal(&parsed_return) {
                issues.push(parse_error_issue(&file, &parsed_return.errors));
                continue;
            }

            let parsed = ParsedView {
                program: &parsed_return.program,
                diagnostics: &parsed_return.errors,
            };

            for check in &self.checks {
                let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
                issues.extend(check.run(&file, &mut ctx));
            }
        }

        for check in &self.checks {
            issues.extend(check.finalize());
        }

        issues.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.span.line.cmp(&b.span.line))
        });

        Ok(issues)
    }
}

/// Build a `Warning.ParseError` issue from oxc diagnostics. We surface the
/// first error verbatim — a fatal parse failure typically cascades, so
/// listing all of them adds noise without information.
fn parse_error_issue(file: &SourceFile, diagnostics: &[oxc_diagnostics::OxcDiagnostic]) -> Issue {
    let message = diagnostics
        .first()
        .map(|d| format!("parse error: {}", d.message))
        .unwrap_or_else(|| "parse error: oxc parser panicked".to_string());

    Issue {
        check_id: "Warning.ParseError".to_string(),
        message,
        file: file.path.clone(),
        // Phase-1: we don't yet map oxc spans into our Span shape (that's
        // the work in cd-81a.2 / A2). Point at line 1 col 1 so formatters
        // produce something coherent; the exact diagnostic location is
        // already in `message`.
        span: Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        },
        priority: Priority(20),
        severity: Severity::Error,
    }
}
