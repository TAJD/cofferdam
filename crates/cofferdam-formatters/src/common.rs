//! Utilities shared across all formatters.

use cofferdam_core::Category;

/// Parse the leading category from a dotted check ID. Cheap, allocation-free.
/// We don't store the category on every `Issue` to keep that struct lean —
/// it's reconstructable from `check_id` and reports are the only consumer.
pub(crate) fn category_of(check_id: &str) -> Option<Category> {
    match check_id.split('.').next()? {
        "Consistency" => Some(Category::Consistency),
        "Design" => Some(Category::Design),
        "Readability" => Some(Category::Readability),
        "Refactor" => Some(Category::Refactor),
        "Warning" => Some(Category::Warning),
        _ => None,
    }
}

/// Lowercase string label for a category. Returns `"unknown"` for `None`.
pub(crate) fn category_str(cat: Option<Category>) -> &'static str {
    match cat {
        Some(Category::Consistency) => "consistency",
        Some(Category::Design) => "design",
        Some(Category::Readability) => "readability",
        Some(Category::Refactor) => "refactor",
        Some(Category::Warning) => "warning",
        // Context-category checks never emit `Issue`s (they run only under
        // `cofferdam context`, never `cofferdam check`), so this arm is
        // unreachable in practice — kept for exhaustiveness.
        Some(Category::Context) => "context",
        None => "unknown",
    }
}

/// Forward-slash normalize. Windows native paths use `\`, but agents and
/// editor protocols universally accept `/` and that's what formatters want.
pub(crate) fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
