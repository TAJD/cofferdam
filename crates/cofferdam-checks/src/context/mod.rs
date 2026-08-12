//! Context providers (`Category::Context`, CD-156) — advisory checks
//! run only by `cofferdam context`, registered via
//! [`crate::all_context_providers`], never [`crate::all_builtins`].
//! Providers land in CP3–CP7 (CD-159..CD-163).

pub mod annotations;
pub mod blast_radius;
pub mod findings;
pub mod knowledge;
pub mod precedent;

use cofferdam_core::{Location, Span};
use std::path::Path;

/// Zero-span placeholder [`Location`] for reporting a whole-file
/// relation where no specific declaration site is being pointed at
/// (CD-230). Mirrors the same fallback pattern `Design.ImportCycle`
/// uses when a cycle-member's import span can't be resolved. Shared
/// by `blast_radius`, `knowledge`, and `findings` — three independent
/// copies of this same zero-span construction accumulated across
/// CD-161/CD-162/CD-220 before being consolidated here.
pub(super) fn whole_file_location(file: &Path) -> Location {
    Location::from_span(
        file,
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::{all_builtins, all_context_providers};
    use cofferdam_core::Category;

    #[test]
    fn provider_ids_are_disjoint_from_builtins_and_all_context_category() {
        let builtin_ids: std::collections::HashSet<&str> =
            all_builtins().iter().map(|c| c.meta().id).collect();
        for p in all_context_providers() {
            assert_eq!(p.meta().category, Category::Context, "{}", p.meta().id);
            assert!(!builtin_ids.contains(p.meta().id), "{}", p.meta().id);
        }
    }
}
