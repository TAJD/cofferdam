//! Digest assembly for `cofferdam context` (CD-158): deterministic
//! ranking, token budgeting with honest truncation, markdown + JSON
//! rendering. Pure functions — all git/engine work happens upstream.

use cofferdam_core::{ChangeSet, ContextItem};

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
}

/// Sort pinned-first, then score desc, then (check_id, title) for
/// deterministic ties; greedily include until the budget is spent.
/// Pinned items are always included (spec: findings + high-priority
/// knowledge can never be evicted by filler).
pub fn assemble(mut items: Vec<ContextItem>, budget: usize) -> Digest {
    items.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.score.cmp(&a.score))
            .then(a.check_id.cmp(&b.check_id))
            .then(a.title.cmp(&b.title))
    });
    let mut included = Vec::new();
    let mut spent = 0usize;
    let mut omitted = 0usize;
    for item in items {
        let cost = item_tokens(&item);
        if item.pinned || spent + cost <= budget {
            spent += cost;
            included.push(item);
        } else {
            omitted += 1;
        }
    }
    Digest {
        included,
        omitted,
        budget,
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
    }
    let payload = Payload {
        schema_version: 1,
        changed_files: changeset.files.iter().map(|p| p.as_path()).collect(),
        items: &digest.included,
        omitted: digest.omitted,
        budget: digest.budget,
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
        };
        let md = render_markdown(&d, 1);
        assert!(md.contains("7 item(s) omitted"));
        assert!(md.contains("--budget"));
    }
}
