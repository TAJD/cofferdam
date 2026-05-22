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
    /// When true, drop baselined findings from the rendered list. The
    /// `(N new, M baselined)` summary line still reports them so the
    /// user can see the gate count is intact. Has no effect when no
    /// baseline is active. Pairs with the CLI `--hide-baselined` flag
    /// (cd-k23 / gh #11). JSON output is intentionally unaffected.
    pub hide_baselined: bool,
}

/// Human-readable text formatter. Groups findings by category, sorts
/// by priority within each, prefixes the priority and severity, and
/// emits ANSI colour by default (configurable via `TextRenderOpts`).
/// The CLI's default output format.
pub struct TextFormatter;

impl TextFormatter {
    /// Render findings with default options.
    pub fn render(issues: &[Issue]) -> String {
        Self::render_with_opts(issues, TextRenderOpts::default())
    }

    /// Render findings honouring the supplied `TextRenderOpts`.
    /// Useful when a caller wants ANSI off, `--hide-baselined`, or
    /// custom truncation.
    pub fn render_with_opts(issues: &[Issue], opts: TextRenderOpts) -> String {
        Self::render_inner(issues.iter().map(|i| (i, false)), issues.len(), None, opts)
    }

    /// Render with per-finding baseline tags. `tagged` is parallel to
    /// the engine's output: each `(Issue, bool)` pair carries `true`
    /// when the finding matched the active baseline.
    pub fn render_with_baseline(tagged: &[(Issue, bool)]) -> String {
        Self::render_with_baseline_opts(tagged, TextRenderOpts::default())
    }

    /// Render with per-finding baseline tags and explicit options.
    /// Findings matching the active baseline render with a
    /// `[baselined]` tag; the run footer breaks out the new vs.
    /// baselined counts so CI can gate on the new total only.
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

        // `--hide-baselined` filters the rendered stream but keeps the
        // pre-filter `total` and `counts` so the summary line still
        // reports the full picture.
        let renderable: Vec<(&Issue, bool)> = if opts.hide_baselined {
            collected.iter().filter(|(_, b)| !*b).copied().collect()
        } else {
            collected.clone()
        };

        // If hiding baselined entries left nothing to render but the
        // run did produce findings, emit a one-line note instead of a
        // bare summary so the terminal does not look empty.
        if renderable.is_empty() && !opts.quiet {
            if let Some(c) = counts {
                let _ = writeln!(
                    out,
                    "\n{} finding(s) ({} new, {} baselined — hidden)",
                    total, c.new, c.baselined
                );
                return out;
            }
        }

        for category in Category::ALL {
            let bucket: Vec<(&Issue, bool)> = renderable
                .iter()
                .filter(|(i, _)| category_of(&i.check_id) == Some(category))
                .copied()
                .collect();
            if bucket.is_empty() {
                continue;
            }

            let _ = writeln!(out, "\n── {:?} ───────────────", category);
            for (issue, baselined) in bucket {
                render_row(&mut out, issue, baselined);
            }
        }

        // Plugin findings whose IDs don't begin with a built-in category
        // prefix (e.g. `Project.X`, `Test.Y`) would otherwise be counted
        // in the summary but never rendered — silently dropping them
        // from the human view is exactly the failure mode of cd-1c7.
        // Surface them under an `Other` heading.
        let other_bucket: Vec<(&Issue, bool)> = renderable
            .iter()
            .filter(|(i, _)| category_of(&i.check_id).is_none())
            .copied()
            .collect();
        if !other_bucket.is_empty() {
            let _ = writeln!(out, "\n── Other ───────────────");
            for (issue, baselined) in other_bucket {
                render_row(&mut out, issue, baselined);
            }
        }

        if !opts.quiet {
            match counts {
                Some(c) => {
                    let suffix = if opts.hide_baselined {
                        " — hidden"
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        out,
                        "\n{} finding(s) ({} new, {} baselined{})",
                        total, c.new, c.baselined, suffix
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

/// Render one finding row. Shared between the per-category buckets and
/// the trailing `Other` bucket so plugin findings render identically to
/// built-ins.
fn render_row(out: &mut String, issue: &Issue, baselined: bool) {
    let tag = if baselined { " [baselined]" } else { "" };
    let _ = writeln!(
        out,
        "  [{:>3}] [{:>8}] {}:{}:{}  {}  ({}){}",
        issue.priority.0,
        issue.severity.as_str(),
        normalize_path(&issue.file),
        issue.location.line(),
        issue.location.column(),
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
                    r.location.line(),
                    r.location.column()
                )
            })
            .collect();
        let _ = writeln!(out, "        also at: {}", locations.join(", "));
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
    use cofferdam_core::{Location, Priority, Severity, Span};
    use std::path::PathBuf;

    fn make_issue(file: PathBuf, check_id: &str) -> Issue {
        Issue {
            location: Location::from_span(
                &file,
                Span {
                    line: 1,
                    column: 5,
                    start_byte: 0,
                    end_byte: 10,
                },
            ),
            file,
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
        let output = TextFormatter::render_with_opts(
            &[issue],
            TextRenderOpts {
                quiet: true,
                ..Default::default()
            },
        );
        assert!(output.contains("Warning.Test"), "findings still print");
        assert!(
            !output.contains("finding(s)"),
            "trailing summary should be suppressed under --quiet, got:\n{output}"
        );
    }

    #[test]
    fn text_formatter_quiet_suppresses_no_findings_line() {
        let output = TextFormatter::render_with_opts(
            &[],
            TextRenderOpts {
                quiet: true,
                ..Default::default()
            },
        );
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
            TextRenderOpts {
                quiet: true,
                ..Default::default()
            },
        );
        assert!(output.contains("Warning.Test"));
        assert!(
            !output.contains("finding(s)"),
            "trailing summary should be suppressed, got:\n{output}"
        );
    }

    #[test]
    fn hide_baselined_drops_baselined_findings_keeps_summary() {
        // One baselined, one new. With --hide-baselined the rendered
        // body must omit the baselined finding but the summary line
        // must still report counts in full (cd-k23 / gh #11).
        let baselined = make_issue(PathBuf::from("src/old.ts"), "Warning.Test");
        let new = make_issue(PathBuf::from("src/new.ts"), "Refactor.Test");
        let output = TextFormatter::render_with_baseline_opts(
            &[(baselined, true), (new, false)],
            TextRenderOpts {
                hide_baselined: true,
                ..Default::default()
            },
        );
        assert!(
            !output.contains("src/old.ts"),
            "baselined finding should be hidden, got:\n{output}"
        );
        assert!(
            !output.contains("[baselined]"),
            "the [baselined] tag should not appear when hidden, got:\n{output}"
        );
        assert!(
            output.contains("src/new.ts"),
            "new finding should still render, got:\n{output}"
        );
        assert!(
            output.contains("2 finding(s) (1 new, 1 baselined"),
            "summary must still report the full count, got:\n{output}"
        );
    }

    #[test]
    fn hide_baselined_with_no_baselined_is_a_no_op() {
        let new = make_issue(PathBuf::from("src/new.ts"), "Refactor.Test");
        let output = TextFormatter::render_with_baseline_opts(
            &[(new, false)],
            TextRenderOpts {
                hide_baselined: true,
                ..Default::default()
            },
        );
        assert!(output.contains("src/new.ts"));
        assert!(output.contains("1 finding(s) (1 new, 0 baselined"));
    }

    #[test]
    fn unknown_category_renders_under_other_bucket() {
        // Plugin findings whose check_id prefix isn't one of the five
        // built-in categories must still render in human output (cd-1c7).
        let plugin = make_issue(PathBuf::from("src/foo.ts"), "Project.UseHelper");
        let output = TextFormatter::render(&[plugin]);
        assert!(
            output.contains("── Other"),
            "uncategorized findings must render under the Other heading; got:\n{output}"
        );
        assert!(
            output.contains("Project.UseHelper"),
            "the plugin check id must appear in the rendered row; got:\n{output}"
        );
    }

    #[test]
    fn hide_baselined_when_all_findings_baselined_emits_short_note() {
        let only = make_issue(PathBuf::from("src/old.ts"), "Warning.Test");
        let output = TextFormatter::render_with_baseline_opts(
            &[(only, true)],
            TextRenderOpts {
                hide_baselined: true,
                ..Default::default()
            },
        );
        assert!(
            !output.contains("src/old.ts"),
            "no findings should render when all are baselined and hidden, got:\n{output}"
        );
        assert!(
            output.contains("1 finding(s) (0 new, 1 baselined"),
            "summary must still print, got:\n{output}"
        );
    }
}
