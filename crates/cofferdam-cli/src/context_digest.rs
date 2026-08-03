//! Digest assembly for `cofferdam context` (CD-158): deterministic
//! ranking, token budgeting with honest truncation, markdown + JSON
//! rendering. Pure functions — all git/engine work happens upstream.

use cofferdam_core::{ChangeSet, ContextItem, RelatedSpan};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

/// CD-241: strip `root` off `path` and forward-slash the remainder so
/// `--format json`'s `changed_files`/`related[].file` are portable
/// across machines/OSes instead of leaking the developer's absolute
/// home-directory path (and matching the "forward-slashed" claim in
/// `docs/reference/context.md`). Falls back to the forward-slashed
/// absolute path when `path` isn't under `root` (shouldn't happen in
/// practice — every path here comes from discovery/`ChangeSet` rooted
/// at `root` — but a lossy display beats a panic).
fn relativize(path: &Path, root_fwd: &str) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    match s.strip_prefix(root_fwd) {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => s,
    }
}

/// Same idea as [`relativize`], but for `root` occurrences embedded in
/// free text (`ContextItem::title`/`body`/`explain`) rather than a bare
/// path — providers format the absolute path directly into prose
/// (`format!("Sibling convention in {}", dir.display())`), so there's
/// no single `Path` value to relativize structurally.
fn relativize_text(text: &str, root_fwd: &str) -> String {
    text.replace('\\', "/").replace(&format!("{root_fwd}/"), "")
}

/// Crude but deterministic token estimate: ceil(chars / 4). Good
/// enough for budget enforcement; NOT a tokenizer.
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

/// CD-239: every field actually rendered must be counted, not just the
/// three original ones — `render_markdown` appends `_why: {explain}_`
/// for every included item, and `render_json` serializes `explain` and
/// `related` (a `{file, location}` pair per entry) along with the rest
/// of the `ContextItem`. Omitting them let real digests cost roughly
/// 3x their accounted budget on an item with even a single `related`
/// span, defeating `--budget`'s purpose and under-reporting
/// `Digest.spent`. `related` is serialized to JSON to estimate its
/// cost since a `Vec<RelatedSpan>` has no single string field to
/// measure directly — this over-counts slightly for the markdown
/// path (which doesn't render `related` at all), but an over-estimate
/// here means the digest budgets conservatively rather than silently
/// exceeding `--budget` again.
fn item_tokens(i: &ContextItem) -> usize {
    let related_tokens = serde_json::to_string(&i.related)
        .map(|s| estimate_tokens(&s))
        .unwrap_or(0);
    estimate_tokens(&i.title)
        + estimate_tokens(&i.body)
        + estimate_tokens(&i.check_id)
        + i.explain.as_deref().map(estimate_tokens).unwrap_or(0)
        + related_tokens
}

/// Total order for presenting items: score desc, then `(check_id,
/// title, body)` as a deterministic tie-break (CD-218) — `body` is the
/// final discriminator since two items can otherwise share both score
/// and title (e.g. two providers pinned at the same
/// `relevance::VERIFIED` anchor, per CD-210).
fn item_order(a: &ContextItem, b: &ContextItem) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then(a.check_id.cmp(&b.check_id))
        .then(a.title.cmp(&b.title))
        .then(a.body.cmp(&b.body))
}

pub struct Digest {
    pub included: Vec<ContextItem>,
    pub omitted: usize,
    pub budget: usize,
    /// Actual tokens spent on `included` — can exceed `budget` when
    /// pinned items (which are never evicted) push the total over.
    pub spent: usize,
}

/// Pinned items (never evicted) are always included first, in score
/// order. The remaining items are grouped by `check_id` — one
/// provider's worth of items per group, each group sorted score desc
/// — and *selected* round-robin: every provider with items left gets
/// one pick per round (highest-scoring group head first) before any
/// provider gets a second pick. This is CD-211: a single high-volume
/// provider (e.g. `Context.BlastRadius` on a busy shared file) can no
/// longer consume the whole budget before a low-volume provider (e.g.
/// `Context.Precedent`) gets a single item considered, which a flat
/// global sort by score allowed. Round-robin governs *selection* only
/// — the selected non-pinned items are re-sorted by [`item_order`]
/// before being appended, so presentation stays relevance-ordered
/// regardless of which round a round-robin pick happened to land in
/// (CD-216).
pub fn assemble(items: Vec<ContextItem>, budget: usize) -> Digest {
    let mut pinned_items = Vec::new();
    let mut groups: BTreeMap<String, Vec<ContextItem>> = BTreeMap::new();
    for item in items {
        if item.pinned {
            pinned_items.push(item);
        } else {
            groups.entry(item.check_id.clone()).or_default().push(item);
        }
    }
    pinned_items.sort_by(item_order);
    for group in groups.values_mut() {
        group.sort_by(item_order);
    }
    let mut groups: Vec<VecDeque<ContextItem>> = groups.into_values().map(VecDeque::from).collect();

    let mut included = Vec::new();
    let mut selected = Vec::new();
    let mut spent = 0usize;
    let mut omitted = 0usize;

    for item in pinned_items {
        spent += item_tokens(&item);
        included.push(item);
    }

    loop {
        let mut turn_order: Vec<usize> = (0..groups.len())
            .filter(|&i| !groups[i].is_empty())
            .collect();
        if turn_order.is_empty() {
            break;
        }
        turn_order.sort_by(|&a, &b| {
            groups[b][0]
                .score
                .cmp(&groups[a][0].score)
                .then(groups[a][0].check_id.cmp(&groups[b][0].check_id))
        });
        for idx in turn_order {
            let item = groups[idx]
                .pop_front()
                .expect("filtered to non-empty above");
            let cost = item_tokens(&item);
            if spent + cost <= budget {
                spent += cost;
                selected.push(item);
            } else {
                omitted += 1;
            }
        }
    }

    selected.sort_by(item_order);
    included.extend(selected);

    Digest {
        included,
        omitted,
        budget,
        spent,
    }
}

pub fn render_markdown(digest: &Digest, changed_file_count: usize, root: &Path) -> String {
    if digest.included.is_empty() {
        return format!("No relevant context found for {changed_file_count} changed file(s).\n");
    }
    let root_fwd = root.to_string_lossy().replace('\\', "/");
    let mut out = format!("# Cofferdam context — {changed_file_count} changed file(s)\n\n");
    for item in &digest.included {
        out.push_str(&format!(
            "## {}  `{}`\n\n{}\n\n",
            relativize_text(&item.title, &root_fwd),
            item.check_id,
            relativize_text(&item.body, &root_fwd)
        ));
        if let Some(explain) = &item.explain {
            out.push_str(&format!(
                "_why: {}_\n\n",
                relativize_text(explain, &root_fwd)
            ));
        }
    }
    if digest.omitted > 0 {
        out.push_str(&format!(
            "_{} item(s) omitted (budget {} tokens); rerun with a larger --budget._\n",
            digest.omitted, digest.budget
        ));
    }
    if digest.spent > digest.budget {
        let pinned_count = digest.included.iter().filter(|i| i.pinned).count();
        out.push_str(&format!(
            "_{} pinned item(s) exceeded the budget by {} tokens._\n",
            pinned_count,
            digest.spent - digest.budget
        ));
    }
    out
}

pub fn render_json(digest: &Digest, changeset: &ChangeSet, root: &Path, pretty: bool) -> String {
    #[derive(serde::Serialize)]
    struct Payload {
        schema_version: u32,
        changed_files: Vec<String>,
        items: Vec<ContextItem>,
        omitted: usize,
        budget: usize,
        /// Actual tokens spent on `items` — additive field (CD-204) so
        /// JSON consumers can detect a pinned-item budget overrun
        /// (`spent > budget`) without re-deriving it from `items`.
        spent: usize,
    }
    let root_fwd = root.to_string_lossy().replace('\\', "/");
    let items = digest
        .included
        .iter()
        .map(|item| ContextItem {
            check_id: item.check_id.clone(),
            title: relativize_text(&item.title, &root_fwd),
            body: relativize_text(&item.body, &root_fwd),
            score: item.score,
            pinned: item.pinned,
            related: item
                .related
                .iter()
                .map(|r| RelatedSpan {
                    file: PathBuf::from(relativize(&r.file, &root_fwd)),
                    location: r.location.clone(),
                })
                .collect(),
            explain: item
                .explain
                .as_deref()
                .map(|e| relativize_text(e, &root_fwd)),
        })
        .collect();
    let payload = Payload {
        schema_version: 1,
        changed_files: changeset
            .files
            .iter()
            .map(|p| relativize(p, &root_fwd))
            .collect(),
        items,
        omitted: digest.omitted,
        budget: digest.budget,
        spent: digest.spent,
    };
    if pretty {
        serde_json::to_string_pretty(&payload).expect("digest serializes")
    } else {
        serde_json::to_string(&payload).expect("digest serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str, score: i32, pinned: bool, body_len: usize) -> ContextItem {
        ContextItem {
            check_id: id.into(),
            title: title.into(),
            body: "x".repeat(body_len),
            score,
            pinned,
            related: vec![],
            explain: None,
        }
    }

    #[test]
    fn assemble_sorts_pinned_then_score_then_stable_ids() {
        let d = assemble(
            vec![
                item("Context.B", "b", 5, false, 4),
                item("Context.A", "a", 5, false, 4),
                item("Context.C", "c", 90, false, 4),
                item("Context.P", "p", 1, true, 4),
            ],
            10_000,
        );
        let order: Vec<&str> = d.included.iter().map(|i| i.check_id.as_str()).collect();
        assert_eq!(
            order,
            vec!["Context.P", "Context.C", "Context.A", "Context.B"]
        );
        assert_eq!(d.omitted, 0);
    }

    #[test]
    fn assemble_evicts_lowest_scored_first_and_counts_omitted() {
        // budget fits roughly two small items
        let d = assemble(
            vec![
                item("Context.A", "a", 90, false, 100),
                item("Context.B", "b", 50, false, 100),
                item("Context.C", "c", 10, false, 100),
            ],
            60,
        ); // each item ≈ (100 body + title + id) / 4 ≈ 27+ tokens
        assert!(d.included.iter().any(|i| i.check_id == "Context.A"));
        assert!(!d.included.iter().any(|i| i.check_id == "Context.C"));
        assert!(d.omitted >= 1);
    }

    #[test]
    fn pinned_items_are_never_evicted_even_over_budget() {
        let d = assemble(
            vec![
                item("Context.P", "p", 1, true, 4000),
                item("Context.A", "a", 99, false, 100),
            ],
            10,
        );
        assert!(d.included.iter().any(|i| i.check_id == "Context.P"));
    }

    /// CD-234: a pinned advisory item sharing both `check_id` and score
    /// with the ordinary items it competes against (as
    /// `Context.Precedent`'s capped-groups summary item does — all
    /// Precedent items share `relevance::FLOOR`) must still survive
    /// budget eviction that drops its unpinned same-score siblings.
    /// Before CD-234 this item was unpinned and had no privileged
    /// position over ordinary Precedent items, so a budget-saturated
    /// digest (exactly the scenario that triggers the cap in the first
    /// place — a monorepo big enough to have 200+ same-kind files) could
    /// silently drop the one item meant to disclose that analysis was
    /// skipped.
    #[test]
    fn a_pinned_same_check_id_same_score_item_survives_when_its_unpinned_siblings_are_evicted() {
        let d = assemble(
            vec![
                item("Context.Precedent", "advisory", 5, true, 100),
                item("Context.Precedent", "convention a", 5, false, 4000),
                item("Context.Precedent", "convention b", 5, false, 4000),
            ],
            50,
        );
        assert!(
            d.included.iter().any(|i| i.title == "advisory"),
            "pinned advisory must survive; got {:?}",
            d.included
        );
        assert!(
            !d.included.iter().any(|i| i.title == "convention a"),
            "unpinned same-score sibling should have been evicted"
        );
    }

    /// CD-216: round-robin governs *selection*, not presentation —
    /// once several providers each contribute multiple items, the
    /// resulting digest must still read in strict relevance order, not
    /// round-by-round pick order.
    #[test]
    fn assemble_presents_selected_items_in_score_order_even_though_selection_was_round_robin() {
        let d = assemble(
            vec![
                item("Context.A", "a1", 95, false, 4),
                item("Context.A", "a2", 94, false, 4),
                item("Context.A", "a3", 93, false, 4),
                item("Context.B", "b1", 70, false, 4),
                item("Context.B", "b2", 69, false, 4),
                item("Context.C", "c1", 5, false, 4),
            ],
            10_000, // fits everything
        );
        let scores: Vec<i32> = d.included.iter().map(|i| i.score).collect();
        let mut sorted = scores.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(
            scores, sorted,
            "digest must read in non-increasing score order regardless of round-robin pick order"
        );
    }

    /// CD-218: two pinned items from different providers sharing both
    /// `score` and `title` (the designed-for case since CD-210 anchors
    /// multiple providers at the same `relevance` constants) must still
    /// assemble deterministically, not fall back to input/collection
    /// order.
    #[test]
    fn assemble_breaks_score_and_title_ties_deterministically_regardless_of_input_order() {
        let mut a = item("Context.Findings", "same title", 95, true, 4);
        a.body = "body-a".into();
        let mut b = item("Context.Knowledge", "same title", 95, true, 4);
        b.body = "body-b".into();

        let d1 = assemble(vec![a.clone(), b.clone()], 10_000);
        let d2 = assemble(vec![b, a], 10_000);

        let ids1: Vec<&str> = d1.included.iter().map(|i| i.check_id.as_str()).collect();
        let ids2: Vec<&str> = d2.included.iter().map(|i| i.check_id.as_str()).collect();
        assert_eq!(
            ids1, ids2,
            "tie-break order must not depend on input vec order"
        );
    }

    /// CD-211: a provider with many items (e.g. `Context.BlastRadius`
    /// on a busy shared file) must not consume the entire budget
    /// before a low-volume provider (e.g. `Context.Precedent`) gets a
    /// single item considered.
    #[test]
    fn assemble_round_robins_across_providers_so_a_high_volume_provider_cannot_starve_others() {
        let mut items = vec![item("Context.Precedent", "p", 5, false, 100)];
        for i in 0..10 {
            items.push(item(
                "Context.BlastRadius",
                &format!("br{i}"),
                90 - i,
                false,
                100,
            ));
        }
        let per_item_cost = item_tokens(&items[0]).max(item_tokens(&items[1]));
        let budget = per_item_cost * 3; // fits ~3 items total, out of 11 offered
        let d = assemble(items, budget);
        assert!(
            d.included.iter().any(|i| i.check_id == "Context.Precedent"),
            "Precedent must get a slot before BlastRadius consumes the whole budget; included={:?}",
            d.included.iter().map(|i| &i.check_id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn item_tokens_counts_explain_and_related() {
        // CD-239: an item with only a title/body/check_id must cost
        // strictly less than the same item once `explain` and
        // `related` are populated — both are actually rendered
        // (markdown appends `_why: {explain}_`; JSON serializes both),
        // so both must be charged.
        let bare = item("Context.A", "a", 5, false, 4);
        let mut with_explain = bare.clone();
        with_explain.explain = Some("a".repeat(200));
        assert!(
            item_tokens(&with_explain) > item_tokens(&bare),
            "a populated explain must increase the item's token cost"
        );

        let mut with_related = bare.clone();
        with_related.related = vec![cofferdam_core::RelatedSpan {
            file: std::path::PathBuf::from("/p/some/file.ts"),
            location: cofferdam_core::Location::from_span(
                std::path::Path::new("/p/some/file.ts"),
                cofferdam_core::Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
            ),
        }];
        assert!(
            item_tokens(&with_related) > item_tokens(&bare),
            "a populated related list must increase the item's token cost"
        );
    }

    #[test]
    fn render_markdown_empty_digest_is_honest() {
        let d = assemble(vec![], 2000);
        let md = render_markdown(&d, 3, Path::new("/repo"));
        assert!(md.contains("No relevant context found for 3 changed file(s)."));
    }

    #[test]
    fn render_markdown_discloses_truncation() {
        let d = Digest {
            included: vec![item("Context.A", "a", 9, false, 4)],
            omitted: 7,
            budget: 2000,
            spent: 3,
        };
        let md = render_markdown(&d, 1, Path::new("/repo"));
        assert!(md.contains("7 item(s) omitted"));
        assert!(md.contains("--budget"));
    }

    #[test]
    fn render_markdown_discloses_pinned_overrun() {
        let d = assemble(
            vec![
                item("Context.P", "p", 1, true, 4000),
                item("Context.A", "a", 99, false, 100),
            ],
            10,
        );
        assert!(d.spent > d.budget);
        let md = render_markdown(&d, 1, Path::new("/repo"));
        assert!(md.contains("pinned item(s) exceeded the budget"));
    }

    #[test]
    fn render_markdown_no_overrun_disclosure_when_within_budget() {
        let d = assemble(vec![item("Context.A", "a", 9, false, 4)], 2000);
        let md = render_markdown(&d, 1, Path::new("/repo"));
        assert!(!md.contains("exceeded the budget"));
    }

    #[test]
    fn relativize_strips_the_root_and_forward_slashes_a_windows_style_absolute_path() {
        let got = relativize(
            Path::new("C:\\Users\\tajdi\\cofferdam\\crates/cofferdam-cli/src/type_host.rs"),
            "C:/Users/tajdi/cofferdam",
        );
        assert_eq!(got, "crates/cofferdam-cli/src/type_host.rs");
    }

    #[test]
    fn relativize_text_strips_every_occurrence_of_the_root_in_free_text() {
        let got = relativize_text(
            "Legacy debt: C:\\repo\\src\\a.ts imports C:\\repo\\src\\b.ts",
            "C:/repo",
        );
        assert_eq!(got, "Legacy debt: src/a.ts imports src/b.ts");
    }

    #[test]
    fn render_json_omits_the_absolute_root_from_changed_files_title_body_explain_and_related() {
        let mut i = item("Context.A", "Legacy debt: C:\\repo\\src\\a.ts", 9, false, 0);
        i.body = "See C:\\repo\\src\\a.ts for detail.".into();
        i.explain = Some("finding in C:\\repo\\src\\a.ts".into());
        i.related = vec![RelatedSpan {
            file: PathBuf::from("C:\\repo\\src\\a.ts"),
            location: cofferdam_core::Location::from_span(
                Path::new("C:\\repo\\src\\a.ts"),
                cofferdam_core::Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
            ),
        }];
        let d = assemble(vec![i], 2000);
        let changeset = ChangeSet {
            files: [PathBuf::from("C:\\repo\\src\\a.ts")].into_iter().collect(),
            line_ranges: Default::default(),
        };
        let json = render_json(&d, &changeset, Path::new("C:\\repo"), false);
        // `location.uri` is a separate, explicitly out-of-scope leak (CD-241
        // note) — everything else this ticket targets must be relativized.
        assert!(json.contains("\"changed_files\":[\"src/a.ts\"]"));
        assert!(json.contains("\"title\":\"Legacy debt: src/a.ts\""));
        assert!(json.contains("\"body\":\"See src/a.ts for detail.\""));
        assert!(json.contains("\"explain\":\"finding in src/a.ts\""));
        assert!(json.contains("\"file\":\"src/a.ts\""));
    }
}
