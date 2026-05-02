//! Human text formatter. Groups by Credo category, then prints
//! priority-sorted findings within each. Mirrors `mix credo`'s output
//! shape so people coming from Elixir feel oriented.

use std::fmt::Write;

use cofferdam_core::{Category, Issue};

pub struct TextFormatter;

impl TextFormatter {
    pub fn render(issues: &[Issue]) -> String {
        let mut out = String::new();

        if issues.is_empty() {
            out.push_str("✓ no findings\n");
            return out;
        }

        for category in Category::ALL {
            let bucket: Vec<&Issue> = issues
                .iter()
                .filter(|i| category_of(&i.check_id) == Some(category))
                .collect();
            if bucket.is_empty() {
                continue;
            }

            let _ = writeln!(out, "\n── {:?} ───────────────", category);
            for issue in bucket {
                let _ = writeln!(
                    out,
                    "  [{:>3}] {}:{}:{}  {}  ({})",
                    issue.priority.0,
                    issue.file.display(),
                    issue.span.line,
                    issue.span.column,
                    issue.message,
                    issue.check_id,
                );
            }
        }

        let _ = writeln!(out, "\n{} finding(s)", issues.len());
        out
    }
}

/// Parse the leading category from a dotted check ID. Cheap, allocation-free.
/// We don't store the category on every `Issue` to keep that struct lean —
/// it's reconstructable from `check_id` and reports are the only consumer.
fn category_of(check_id: &str) -> Option<Category> {
    match check_id.split('.').next()? {
        "Consistency" => Some(Category::Consistency),
        "Design" => Some(Category::Design),
        "Readability" => Some(Category::Readability),
        "Refactor" => Some(Category::Refactor),
        "Warning" => Some(Category::Warning),
        _ => None,
    }
}
