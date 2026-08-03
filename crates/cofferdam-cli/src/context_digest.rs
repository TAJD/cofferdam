//! Digest assembly for `cofferdam context` (CD-158): deterministic
//! ranking, token budgeting with honest truncation, markdown + JSON
//! rendering. Pure functions — all git/engine work happens upstream.

use cofferdam_core::{ChangeSet, ContextItem};
use std::collections::{BTreeMap, VecDeque};

/// Crude but deterministic token estimate: ceil(chars / 4). Good
/// enough for budget enforcement; NOT a tokenizer.
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

fn item_tokens(i: &ContextItem) -> usize {
    estimate_tokens(&i.title) + estimate_tokens(&i.body) + estimate_tokens(&i.check_id)
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
/// — and included round-robin: every provider with items left gets one
/// pick per round (highest-scoring group head first) before any
/// provider gets a second pick. This is CD-211: a single high-volume
/// provider (e.g. `Context.BlastRadius` on a busy shared file) can no
/// longer consume the whole budget before a low-volume provider (e.g.
/// `Context.Precedent`) gets a single item considered, which a flat
/// global sort by score allowed.
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
    pinned_items.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    for group in groups.values_mut() {
        group.sort_by(|a, b| b.score.cmp(&a.score).then(a.title.cmp(&b.title)));
    }
    let mut groups: Vec<VecDeque<ContextItem>> = groups.into_values().map(VecDeque::from).collect();

    let mut included = Vec::new();
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
                included.push(item);
            } else {
                omitted += 1;
            }
        }
    }

    Digest {
        included,
        omitted,
        budget,
        spent,
    }
}

pub fn render_markdown(digest: &Digest, changed_file_count: usize) -> String {
    if digest.included.is_empty() {
        return format!("No relevant context found for {changed_file_count} changed file(s).\n");
    }
    let mut out = format!("# Cofferdam context — {changed_file_count} changed file(s)\n\n");
    for item in &digest.included {
        out.push_str(&format!(
            "## {}  `{}`\n\n{}\n\n",
            item.title, item.check_id, item.body
        ));
        if let Some(explain) = &item.explain {
            out.push_str(&format!("_why: {explain}_\n\n"));
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

pub fn render_json(digest: &Digest, changeset: &ChangeSet, pretty: bool) -> String {
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        schema_version: u32,
        changed_files: Vec<&'a std::path::Path>,
        items: &'a [ContextItem],
        omitted: usize,
        budget: usize,
        /// Actual tokens spent on `items` — additive field (CD-204) so
        /// JSON consumers can detect a pinned-item budget overrun
        /// (`spent > budget`) without re-deriving it from `items`.
        spent: usize,
    }
    let payload = Payload {
        schema_version: 1,
        changed_files: changeset.files.iter().map(|p| p.as_path()).collect(),
        items: &digest.included,
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
    fn render_markdown_empty_digest_is_honest() {
        let d = assemble(vec![], 2000);
        let md = render_markdown(&d, 3);
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
        let md = render_markdown(&d, 1);
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
        let md = render_markdown(&d, 1);
        assert!(md.contains("pinned item(s) exceeded the budget"));
    }

    #[test]
    fn render_markdown_no_overrun_disclosure_when_within_budget() {
        let d = assemble(vec![item("Context.A", "a", 9, false, 4)], 2000);
        let md = render_markdown(&d, 1);
        assert!(!md.contains("exceeded the budget"));
    }
}
