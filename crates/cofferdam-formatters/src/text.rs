//! Human text formatter. Groups findings by category, then prints them
//! priority-sorted within each.
//!
//! Color: this formatter intentionally emits no ANSI escape codes today.
//! That makes the output trivially honor `NO_COLOR` (the env var has no
//! effect because there is nothing to suppress) and keeps the output
//! grep-friendly. If color is added later, gate it on
//! `std::env::var_os("NO_COLOR").is_none()` per <https://no-color.org>.

use std::fmt::Write;

use cofferdam_core::{Category, Issue};

/// Options for the text formatter. Defaults match the historical
/// behavior — full output with the trailing summary line.
#[derive(Copy, Clone, Debug, Default)]
pub struct TextRenderOpts {
    /// When true, suppress the trailing `N finding(s)` summary line.
    /// Findings themselves still print. Pairs with the CLI `--quiet`
    /// flag.
    pub quiet: bool,
}

pub struct TextFormatter;

impl TextFormatter {
    pub fn render(issues: &[Issue]) -> String {
        Self::render_with_opts(issues, TextRenderOpts::default())
    }

    pub fn render_with_opts(issues: &[Issue], opts: TextRenderOpts) -> String {
        Self::render_inner(issues.iter().map(|i| (i, false)), issues.len(), None, opts)
    }

    /// Render with per-finding baseline tags. `tagged` is parallel to
    /// the engine's output: each `(Issue, bool)` pair carries `true`
    /// when the finding matched the active baseline.
    pub fn render_with_baseline(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_opts(tagged, TextRenderOpts::default())
    }

    pub fn render_with_baseline_opts(tagged: &[(Issue, bool)], opts: TextRenderOpts) -> String {
        let baselined = tagged.iter().filter(|(_, b)| *b).count();
        Self::render_inner(
            tagged.iter().map(|(i, b)| (i, *b)),
            tagged.len(),
            Some(BaselineCounts {
                baselined,
                new: tagged.len() - baselined,
            }),
            opts,
        )
    }

    fn render_inner<'a, I>(
        items: I,
        total: usize,
        counts: Option<BaselineCounts>,
        opts: TextRenderOpts,
    ) -> String
    where
        I: IntoIterator<Item = (&'a Issue, bool)>,
    {
        let mut out = String::new();

        let collected: Vec<(&Issue, bool)> = items.into_iter().collect();

        if collected.is_empty() {
            if !opts.quiet {
                out.push_str("✓ no findings\n");
            }
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

        if !opts.quiet {
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

    #[test]
    fn text_formatter_quiet_suppresses_trailing_summary() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = TextFormatter::render_with_opts(&[issue], TextRenderOpts { quiet: true });
        assert!(output.contains("Warning.Test"), "findings still print");
        assert!(
            !output.contains("finding(s)"),
            "trailing summary should be suppressed under --quiet, got:\n{output}"
        );
    }

    #[test]
    fn text_formatter_quiet_suppresses_no_findings_line() {
        let output = TextFormatter::render_with_opts(&[], TextRenderOpts { quiet: true });
        assert!(
            !output.contains("no findings"),
            "empty-result message should be suppressed under --quiet, got:\n{output}"
        );
    }

    #[test]
    fn text_formatter_quiet_baseline_suppresses_trailing_summary() {
        let issue = make_issue(PathBuf::from("src/foo.ts"), "Warning.Test");
        let output = TextFormatter::render_with_baseline_opts(
            &[(issue, false)],
            TextRenderOpts { quiet: true },
        );
        assert!(output.contains("Warning.Test"));
        assert!(
            !output.contains("finding(s)"),
            "trailing summary should be suppressed, got:\n{output}"
        );
    }
}
