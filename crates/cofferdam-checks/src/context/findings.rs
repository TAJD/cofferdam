//! `Context.Findings` (CD-159) — delta-scoped check summary + legacy-debt
//! rollup for `cofferdam context`.
//!
//! Reads the `ALL_PRE_FILTER_FINDINGS` corpus slot, which the engine
//! rebuilds (from the union of `run`/`pass2`/finalize-Phase-A findings)
//! immediately before invoking Context-category providers — see the
//! finalize block in `cofferdam-engine::Engine`'s analyze pipeline.
//! Each changed file's findings are split by whether they land on a line
//! the diff actually touched (`ChangeSet::line_ranges`):
//!
//! - Findings on a changed line are "fresh" — summarized in one digest
//!   item, grouped and counted by check id.
//! - Findings elsewhere in a changed file are "legacy" — rolled into one
//!   line per file, never listed individually, per the product spec's
//!   baseline policy ("nothing hidden, nothing noisy").
//!
//! A file with no `line_ranges` entry (explicit-files mode — see
//! `ChangeSet` docs) is treated as "whole file changed": every finding
//! in it counts as fresh, none legacy.
//!
//! ## Why line ranges, not `.cofferdam/baseline.json`
//!
//! The wiki's baseline/suppression policy section borrows the word
//! "baseline" from cofferdam's CI-gate baseline feature
//! (`.cofferdam/baseline.json`, `cofferdam-engine::baseline`). That
//! mechanism keys findings by a SHA-256 of their exact span text and
//! lives entirely in `cofferdam-engine` + the CLI (`cofferdam-checks`
//! intentionally has no dependency on `cofferdam-engine` — see that
//! crate's module docs) — and the `ALL_PRE_FILTER_FINDINGS` corpus slot
//! this provider can see only carries `(check_id, line)` pairs, not the
//! byte-exact span text a real baseline signature needs. Wiring true
//! `.cofferdam/baseline.json` awareness through to Context providers is
//! follow-up work beyond CP3 (flag for the CD-156 epic). This provider's
//! "legacy debt" instead means "pre-existing in a file the diff touched,
//! but not on a line the diff changed" — the same product framing
//! (surface it, don't hide it, don't enumerate it), built from data the
//! Context-provider corpus actually exposes today.

use std::collections::BTreeMap;
use std::path::Path;

use cofferdam_core::{
    relevance, Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, FinalizeContext,
    Issue, Location, RelatedSpan, Severity, SourceFile, Uri, ALL_PRE_FILTER_FINDINGS,
};

pub struct Findings;

const META: CheckMeta = CheckMeta {
    id: "Context.Findings",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "Delta-scoped summary of check findings in the changeset: fresh findings grouped by check id, plus a per-file legacy-debt rollup for pre-existing findings outside the changed lines.",
    body: include_str!("../../docs/Context.Findings.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // context_items() reads ctx.corpus (ALL_PRE_FILTER_FINDINGS) — not a
    // pure function of (file content, options).
    pure_run: false,
};

impl Check for Findings {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // This provider emits nothing from the ordinary per-file pass —
        // all its output comes from context_items() reading the corpus
        // snapshot the engine builds for it.
        Vec::new()
    }

    fn context_items(
        &self,
        changeset: &ChangeSet,
        ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        let mut fresh_counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut fresh_total: u32 = 0;
        let mut legacy_items: Vec<ContextItem> = Vec::new();

        ctx.corpus.with_slot(&ALL_PRE_FILTER_FINDINGS, |slot| {
            for file in &changeset.files {
                let Some(findings) = slot.get(file) else {
                    continue;
                };
                let ranges = changeset.line_ranges.get(file);
                let mut legacy_count: u32 = 0;
                for (check_id, line) in findings {
                    let is_fresh = match ranges {
                        None => true,
                        Some(rs) => rs.iter().any(|r| *line >= r.start && *line <= r.end),
                    };
                    if is_fresh {
                        *fresh_counts.entry(check_id.clone()).or_insert(0) += 1;
                        fresh_total += 1;
                    } else {
                        legacy_count += 1;
                    }
                }
                if legacy_count > 0 {
                    legacy_items.push(legacy_item(file, legacy_count));
                }
            }
        });

        let mut items = legacy_items;
        if fresh_total > 0 {
            items.push(fresh_summary_item(fresh_total, &fresh_counts));
        }
        items
    }
}

fn fresh_summary_item(total: u32, counts: &BTreeMap<String, u32>) -> ContextItem {
    let breakdown = counts
        .iter()
        .map(|(id, n)| format!("{n}\u{d7} {id}"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if total == 1 { "finding" } else { "findings" };
    let check_ids: Vec<&str> = counts.keys().map(String::as_str).collect();
    ContextItem {
        check_id: META.id.into(),
        title: format!("{total} {noun} in changed lines"),
        body: format!("{breakdown} \u{2014} run `cofferdam check` for detail."),
        score: relevance::VERIFIED,
        pinned: true,
        related: vec![],
        explain: Some(format!(
            "{total} finding(s) from {} on line(s) the diff changed",
            check_ids.join(", ")
        )),
    }
}

fn legacy_item(file: &Path, count: u32) -> ContextItem {
    let noun = if count == 1 { "finding" } else { "findings" };
    ContextItem {
        check_id: META.id.into(),
        title: format!("Legacy debt: {}", file.display()),
        body: format!("This file carries {count} baselined {noun} outside the changed lines."),
        score: relevance::INDIRECT,
        pinned: true,
        // CD-220: a whole-file pointer (no specific span — `count` rolls
        // up findings across the file) so `[[context_suppress]]` rules
        // scoped by path can actually match this item; previously
        // `related: vec![]` made path-scoped suppression of legacy-debt
        // items a permanent no-op.
        related: vec![RelatedSpan {
            file: file.to_path_buf(),
            location: Location::bytes(Uri::from_path(file), 0, 0, 1, 1),
        }],
        explain: Some(format!(
            "{count} pre-existing finding(s) in {} outside the lines the diff changed",
            file.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{CorpusIndex, LineRange};
    use std::path::PathBuf;

    fn setup_corpus(entries: &[(&str, &str, u32)]) -> CorpusIndex {
        let corpus = CorpusIndex::default();
        corpus.with_slot(&ALL_PRE_FILTER_FINDINGS, |slot| {
            for (file, check_id, line) in entries {
                slot.entry(PathBuf::from(file))
                    .or_default()
                    .push((check_id.to_string(), *line));
            }
        });
        corpus
    }

    #[test]
    fn all_context_provider_items_are_pinned() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Warning.TripleEquals", 5),
            ("/a.ts", "Warning.NoConsoleLog", 20),
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 2, "one fresh summary + one legacy rollup");
        assert!(items.iter().all(|i| i.pinned), "items: {items:?}");
    }

    #[test]
    fn fresh_findings_summarized_with_counts_by_check_id() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Refactor.PreferConstOverLet", 3),
            ("/a.ts", "Refactor.PreferConstOverLet", 4),
            ("/a.ts", "Warning.NoConsoleLog", 6),
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.check_id, "Context.Findings");
        assert_eq!(item.title, "3 findings in changed lines");
        assert!(
            item.body.contains("2\u{d7} Refactor.PreferConstOverLet"),
            "body: {}",
            item.body
        );
        assert!(
            item.body.contains("1\u{d7} Warning.NoConsoleLog"),
            "body: {}",
            item.body
        );
        assert!(item.body.contains("cofferdam check"));
    }

    #[test]
    fn legacy_findings_are_rolled_up_not_listed_individually() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Warning.TripleEquals", 50),
            ("/a.ts", "Warning.NoConsoleLog", 60),
            ("/a.ts", "Warning.NoConsoleLog", 70),
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 1, "only the rollup, no per-finding items");
        let item = &items[0];
        assert!(item.title.contains("Legacy debt"));
        assert!(item.body.contains("3 baselined findings"), "{}", item.body);
        // No individual check ids leaked into the rollup body.
        assert!(!item.body.contains("Warning.TripleEquals"));
    }

    #[test]
    fn legacy_item_related_points_at_its_own_file() {
        // CD-220: a path-scoped `[[context_suppress]]` rule matches an
        // item via `item.related` — a legacy-debt item with no related
        // spans could never be suppressed by path.
        let corpus = setup_corpus(&[("/a.ts", "Warning.TripleEquals", 50)]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].related.len(), 1);
        assert_eq!(items[0].related[0].file, PathBuf::from("/a.ts"));
    }

    #[test]
    fn mixed_fresh_and_legacy_produce_two_distinct_signals() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Warning.TripleEquals", 5),  // in range: fresh
            ("/a.ts", "Warning.NoConsoleLog", 99), // out of range: legacy
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .any(|i| i.title.contains("1 finding in changed lines")));
        assert!(items.iter().any(|i| i.title.contains("Legacy debt")));
    }

    #[test]
    fn explicit_file_mode_with_no_line_ranges_treats_whole_file_as_fresh() {
        let corpus = setup_corpus(&[("/a.ts", "Warning.TripleEquals", 500)]);
        let cs = ChangeSet::from_files([PathBuf::from("/a.ts")]);
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 1);
        assert!(items[0].title.contains("in changed lines"));
    }

    #[test]
    fn no_findings_for_changeset_files_yields_no_items() {
        let corpus = setup_corpus(&[("/unrelated.ts", "Warning.TripleEquals", 1)]);
        let cs = ChangeSet::from_files([PathBuf::from("/a.ts")]);
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert!(items.is_empty());
    }

    #[test]
    fn multiple_files_each_get_their_own_legacy_rollup() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Warning.TripleEquals", 500),
            ("/b.ts", "Warning.NoConsoleLog", 500),
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts"), PathBuf::from("/b.ts")]
                .into_iter()
                .collect(),
            line_ranges: [
                (
                    PathBuf::from("/a.ts"),
                    vec![LineRange { start: 1, end: 10 }],
                ),
                (
                    PathBuf::from("/b.ts"),
                    vec![LineRange { start: 1, end: 10 }],
                ),
            ]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|i| i.title.ends_with("a.ts")));
        assert!(items.iter().any(|i| i.title.ends_with("b.ts")));
    }

    #[test]
    fn all_context_provider_items_populate_explain() {
        let corpus = setup_corpus(&[
            ("/a.ts", "Warning.TripleEquals", 5),
            ("/a.ts", "Warning.NoConsoleLog", 99),
        ]);
        let cs = ChangeSet {
            files: [PathBuf::from("/a.ts")].into_iter().collect(),
            line_ranges: [(
                PathBuf::from("/a.ts"),
                vec![LineRange { start: 1, end: 10 }],
            )]
            .into_iter()
            .collect(),
        };
        let mut fctx = FinalizeContext::new(&corpus);
        let items = Findings.context_items(&cs, &mut fctx);
        assert_eq!(items.len(), 2);
        assert!(
            items.iter().all(|i| i.explain.is_some()),
            "items: {items:?}"
        );
    }

    #[test]
    fn run_never_emits_ordinary_issues() {
        let file = SourceFile::new(PathBuf::from("/a.ts"), "const x = 1;\n".to_string());
        let mut ctx = CheckContext::new(&file);
        assert!(Findings.run(&file, &mut ctx).is_empty());
    }
}
