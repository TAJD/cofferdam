//! Human text formatter. Groups findings by category, then prints them
//! priority-sorted within each.

use std::fmt::Write;

use cofferdam_core::{Category, Issue};

pub struct TextFormatter;

impl TextFormatter {
    pub fn render(issues: &[Issue]) -> String {
        Self::render_inner(issues.iter().map(|i| (i, false)), issues.len(), None)
    }

    /// Render with per-finding baseline tags. `tagged` is parallel to
    /// the engine's output: each `(Issue, bool)` pair carries `true`
    /// when the finding matched the active baseline.
    pub fn render_with_baseline(tagged: &[(Issue, bool)]) -> String {
        let baselined = tagged.iter().filter(|(_, b)| *b).count();
        Self::render_inner(
            tagged.iter().map(|(i, b)| (i, *b)),
            tagged.len(),
            Some(BaselineCounts {
                baselined,
                new: tagged.len() - baselined,
            }),
        )
    }

    fn render_inner<'a, I>(items: I, total: usize, counts: Option<BaselineCounts>) -> String
    where
        I: IntoIterator<Item = (&'a Issue, bool)>,
    {
        let mut out = String::new();

        let collected: Vec<(&Issue, bool)> = items.into_iter().collect();

        if collected.is_empty() {
            out.push_str("✓ no findings\n");
            return out;
        }

        for category in Category::ALL {
            let bucket: Vec<(&Issue, bool)> = collected
                .iter()
                .filter(|(i, _)| category_of(&i.check_id) == Some(category))
                .copied()
                .collect();
            if bucket.is_empty() {
                continue;
            }

            let _ = writeln!(out, "\n── {:?} ───────────────", category);
            for (issue, baselined) in bucket {
                let tag = if baselined { " [baselined]" } else { "" };
                let _ = writeln!(
                    out,
                    "  [{:>3}] [{:>8}] {}:{}:{}  {}  ({}){}",
                    issue.priority.0,
                    issue.severity.as_str(),
                    normalize_path(&issue.file),
                    issue.span.line,
                    issue.span.column,
                    issue.message,
                    issue.check_id,
                    tag,
                );
                if !issue.related.is_empty() {
                    let locations: Vec<String> = issue
                        .related
                        .iter()
                        .map(|r| {
                            format!(
                                "{}:{}:{}",
                                normalize_path(&r.file),
                                r.span.line,
                                r.span.column
                            )
                        })
                        .collect();
                    let _ = writeln!(out, "        also at: {}", locations.join(", "));
                }
            }
        }

        match counts {
            Some(c) => {
                let _ = writeln!(
                    out,
                    "\n{} finding(s) ({} new, {} baselined)",
                    total, c.new, c.baselined
                );
            }
            None => {
                let _ = writeln!(out, "\n{} finding(s)", total);
            }
        }
        out
    }
}

#[derive(Copy, Clone)]
struct BaselineCounts {
    baselined: usize,
    new: usize,
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

/// Forward-slash normalize. Windows native paths use `\`, but this ensures
/// consistent output across all platforms for better readability and copy-paste compatibility.
fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Priority, Severity, Span};
    use std::path::PathBuf;

    fn make_issue(file: PathBuf, check_id: &str) -> Issue {
        Issue {
            file,
            span: Span {
                line: 1,
                column: 5,
                start_byte: 0,
                end_byte: 10,
            },
            message: "test message".into(),
            check_id: check_id.into(),
            severity: Severity::Medium,
            priority: Priority(10),
            related: Vec::new(),
        }
    }

    #[test]
    fn text_formatter_normalizes_windows_paths() {
        let issue = make_issue(PathBuf::from(r"C:\Users\demo\src\foo.ts"), "Warning.Test");
        let output = TextFormatter::render(&[issue]);
        assert!(output.contains("C:/Users/demo/src/foo.ts"));
        assert!(!output.contains(r"\Users"));
    }

    #[test]
    fn text_formatter_preserves_forward_slash_paths() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = TextFormatter::render(&[issue]);
        assert!(output.contains("src/foo.ts"));
    }

    #[test]
    fn text_formatter_empty_findings() {
        let output = TextFormatter::render(&[]);
        assert!(output.contains("✓ no findings"));
    }
}
