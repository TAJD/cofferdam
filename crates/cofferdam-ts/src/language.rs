//! `Language` impl for TypeScript — the only adapter today.
//!
//! See `cofferdam_core::language::Language` for the trait contract.
//! `TypeScript` is a unit struct used as a type parameter for
//! `Engine<L>`, `Check<L>`, and `CheckContext<'_, L>`. The CLI binary
//! wires it in; future cofferdam-py would add a sibling `Python` here
//! (or in its own crate) without touching cofferdam-core or
//! cofferdam-engine.

use cofferdam_core::{Language, ParseOutcome, SourceFile};
use oxc_allocator::Allocator;

use crate::parser::{diagnostic_messages, parse_fatal, parse_into, ParsedView};

/// TypeScript language adapter — owns the oxc parser and AST surface.
pub struct TypeScript;

impl Language for TypeScript {
    type Parsed<'a> = ParsedView<'a>;

    fn name() -> &'static str {
        "typescript"
    }

    fn default_extensions() -> &'static [&'static str] {
        &["ts", "tsx", "mts", "cts"]
    }

    fn with_parsed<R>(file: &SourceFile, f: impl FnOnce(ParseOutcome<Self::Parsed<'_>>) -> R) -> R {
        // Per-file allocator: lives until this function returns. The
        // parsed view borrows from it; the closure runs while it's
        // still alive.
        let allocator = Allocator::default();
        let parsed_return = parse_into(&allocator, file);
        let diagnostics = diagnostic_messages(&parsed_return.errors);

        if parse_fatal(&parsed_return) {
            return f(ParseOutcome {
                parsed: None,
                diagnostics,
            });
        }

        let view = ParsedView {
            program: &parsed_return.program,
            diagnostics: &parsed_return.errors,
        };
        f(ParseOutcome {
            parsed: Some(view),
            diagnostics,
        })
    }
}
