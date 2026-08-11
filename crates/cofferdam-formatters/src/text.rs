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

use crate::common::{category_of, normalize_path};

/// Options for the text formatter. Defaults match the historical
/// behavior — full output with the trailing summary line.
#[derive(Copy, Clone, Debug, Default)]
pub struct TextRenderOpts {
    /// When true, suppress the trailing `N finding(s)` summary line.
    /// Findings themselves still print. Pairs with the CLI `--quiet`
    /// flag.
    pub quiet: bool,
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
        Self::render_with_baseline_filtered(tagged, tagged, opts)
    }

    /// Render a caller-filtered slice of findings (`rendered`) while
    /// still reporting summary totals from the pre-filter `full` slice.
    /// Lets the CLI apply `--hide-baselined` once, ahead of the
    /// per-format `match`, and have every formatter agree on the same
    /// counts (cd-315) rather than each formatter re-deriving them from
    /// whatever subset it happens to be handed.
    pub fn render_with_baseline_filtered(
        full: &[(Issue, bool)],
        rendered: &[(Issue, bool)],
        opts: TextRenderOpts,
    ) -> String {
        let baselined = full.iter().filter(|(_, b)| *b).count();
        Self::render_inner(
            rendered.iter().map(|(i, b)| (i, *b)),
            full.len(),
            Some(BaselineCounts {
                baselined,
                new: full.len() - baselined,
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

        if total == 0 {
            if !opts.quiet {
                out.push_str("✓ no findings\n");
            }
            return out;
        }

        // The caller may have already filtered `collected` down (e.g.
        // `--hide-baselined` stripping baselined entries) while `total`
        // and `counts` still reflect the pre-filter run. If that left
        // nothing to render, emit a one-line note instead of a bare
        // summary so the terminal does not look empty.
        if collected.is_empty() && !opts.quiet {
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
                render_row(&mut out, issue, baselined);
            }
        }

        // Plugin findings whose IDs don't begin with a built-in category
        // prefix (e.g. `Project.X`, `Test.Y`) would otherwise be counted
        // in the summary but never rendered — silently dropping them
        // from the human view is exactly the failure mode of cd-1c7.
        // Surface them under an `Other` heading.
        let other_bucket: Vec<(&Issue, bool)> = collected
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
                    // `collected` is narrower than the pre-filter total
                    // only when the caller applied `--hide-baselined`.
                    let suffix = if collected.len() < total {
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

    #[test]
    fn hide_baselined_drops_baselined_findings_keeps_summary() {
        // One baselined, one new. With --hide-baselined the CLI passes a
        // `rendered` slice that already omits the baselined finding, but
        // `full` still reports counts in full (cd-k23 / gh #11 / cd-315).
        let baselined = make_issue(PathBuf::from("src/old.ts"), "Warning.Test");
        let new = make_issue(PathBuf::from("src/new.ts"), "Refactor.Test");
        let full = [(baselined, true), (new.clone(), false)];
        let rendered = [(new, false)];
        let output = TextFormatter::render_with_baseline_filtered(
            &full,
            &rendered,
            TextRenderOpts::default(),
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
        let full = [(new, false)];
        let output =
            TextFormatter::render_with_baseline_filtered(&full, &full, TextRenderOpts::default());
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
        let full = [(only, true)];
        let output =
            TextFormatter::render_with_baseline_filtered(&full, &[], TextRenderOpts::default());
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
