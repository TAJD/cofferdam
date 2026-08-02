//! Context providers (`Category::Context`, CD-156) — advisory checks
//! run only by `cofferdam context`, registered via
//! [`crate::all_context_providers`], never [`crate::all_builtins`].
//! Providers land in CP3–CP7 (CD-159..CD-163).

pub mod blast_radius;
pub mod findings;
pub mod knowledge;
pub mod precedent;

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
