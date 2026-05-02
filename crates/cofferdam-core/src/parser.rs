//! TypeScript parser — thin wrapper over oxc.
//!
//! Why this lives in `cofferdam-core`: every check that wants AST access
//! takes a `&CheckContext`, which exposes the parsed Program. Putting the
//! parser elsewhere means CheckContext can't carry the right type.
//!
//! Lifetime model:
//! - oxc allocates the AST inside an `oxc_allocator::Allocator` (a
//!   bumpalo arena). The `Program<'a>` borrows from the arena.
//! - Engine owns one `ParsedFile` per file being analyzed. ParsedFile
//!   owns the Allocator + Program together via `self_cell`-style pairing
//!   (here we use a small unsafe-free pattern: pin the allocator behind
//!   a Box, build the Program against it, store both in the same struct).
//! - We *could* pull in `self_cell` or `ouroboros` for this, but the
//!   pattern is small enough that the extra dep isn't worth it. See
//!   `parse_into` below.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{ParseOptions, Parser, ParserReturn};
use oxc_span::SourceType;

use crate::source::SourceFile;

/// Pick the correct oxc `SourceType` from a file path.
///
/// `.ts` / `.mts` / `.cts` → TypeScript, no JSX
/// `.tsx` → TypeScript with JSX
/// `.d.ts` / `.d.mts` / `.d.cts` → TypeScript declaration files
/// Anything else → falls back to TS.
pub fn source_type_for(path: &Path) -> SourceType {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
        return SourceType::ts().with_typescript_definition(true);
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => SourceType::tsx(),
        // .mts / .cts: TS module/CJS — there's no dedicated SourceType
        // constructor for these, so use ts(). Module mode is decided by
        // oxc from the AST, and for syntactic checks the difference
        // doesn't matter.
        _ => SourceType::ts(),
    }
}

/// Parse a source file's text inside the given allocator.
///
/// The caller owns the allocator and the resulting Program borrows from
/// it; both must live for the duration of any check that touches the
/// AST. The engine handles this lifetime by creating a fresh allocator
/// per file, parsing, running checks, and dropping.
pub fn parse_into<'a>(allocator: &'a Allocator, file: &'a SourceFile) -> ParserReturn<'a> {
    let source_type = source_type_for(&file.path);
    Parser::new(allocator, &file.text, source_type)
        .with_options(ParseOptions {
            // Keep panic-mode parsing on so we still get a (partial) AST
            // when there are errors — many checks can still operate on
            // the recovered tree, and we only want to *suppress* other
            // checks for the file when oxc returned no AST at all.
            allow_return_outside_function: false,
            ..ParseOptions::default()
        })
        .parse()
}

/// Wrap a list of oxc diagnostics into human-readable parse-error messages.
pub fn diagnostic_messages(diagnostics: &[OxcDiagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.message.to_string()).collect()
}

/// True if the parse return represents a fatal parse failure — i.e. oxc
/// gave up early and the AST is unusable. With panic-mode recovery on,
/// most TS files produce *some* AST even with errors; we only treat
/// `panicked == true` as fatal.
pub fn parse_fatal(parsed: &ParserReturn<'_>) -> bool {
    parsed.panicked
}

/// Borrowed view exposed to checks: the parsed Program plus parse
/// diagnostics. `Program` is the oxc AST root.
pub struct ParsedView<'a> {
    pub program: &'a Program<'a>,
    pub diagnostics: &'a [OxcDiagnostic],
}
