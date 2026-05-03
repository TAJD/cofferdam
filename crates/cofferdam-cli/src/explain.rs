//! `cofferdam explain <check_id>` — surface a check's metadata and
//! prose explanation in the terminal.
//!
//! With `--full`, also prints the companion markdown body from
//! `CheckMeta::body` (extracted from `crates/cofferdam-checks/docs/`).
//! The frontmatter block (`---…---`) is stripped before terminal output.
//! In `--robot` mode the body is included as a `body` JSON field.

use std::fmt::Write as _;
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Category, CheckMeta, OptionDefault, OptionSpec};
use serde::Serialize;

pub struct ExplainArgs {
    pub check_id: String,
    pub robot: bool,
    pub pretty: bool,
    pub full: bool,
}

pub fn run(args: ExplainArgs) -> ExitCode {
    let ExplainArgs {
        check_id,
        robot,
        pretty,
        full,
    } = args;

    // `Vec<&'static CheckMeta>`: cheap, the metas are static.
    let metas: Vec<&'static CheckMeta> = all_builtins().iter().map(|c| c.meta()).collect();

    let Some(meta) = metas.iter().find(|m| m.id == check_id).copied() else {
        return not_found(&check_id, &metas, robot);
    };

    if robot {
        let report = build_report(meta, full);
        let s = if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
        .expect("ExplainReport serializes infallibly");
        println!("{}", s);
    } else {
        print!("{}", render_text(meta));
        if full {
            println!("\n---\n");
            print!("{}", strip_frontmatter(meta.body));
        }
    }
    ExitCode::SUCCESS
}

/// Print "no such check" plus a short shortlist of close-ish IDs (or
/// the full list if no near match). Exit 2 — same convention used
/// elsewhere in the CLI for usage / "input was wrong" failures.
fn not_found(query: &str, metas: &[&'static CheckMeta], robot: bool) -> ExitCode {
    if robot {
        // Schema-stable error shape: `{ "error": "...", "suggestions": [...] }`.
        // Agents can branch on `error` without parsing prose.
        #[derive(Serialize)]
        struct ErrReport<'a> {
            error: String,
            suggestions: Vec<&'a str>,
        }
        let suggestions = suggestions_for(query, metas);
        let report = ErrReport {
            error: format!("no such check: {query}"),
            suggestions: suggestions.iter().map(|m| m.id).collect(),
        };
        let s = serde_json::to_string(&report).expect("ErrReport serializes infallibly");
        // Errors go to stderr; the `--robot` JSON contract is stdout-
        // for-success-only, and this is a usage error.
        eprintln!("{}", s);
        return ExitCode::from(2);
    }

    eprintln!("error: no such check: {query}");
    let suggestions = suggestions_for(query, metas);
    if !suggestions.is_empty() {
        eprintln!();
        eprintln!("did you mean:");
        for m in &suggestions {
            eprintln!("  {}", m.id);
        }
    } else {
        eprintln!();
        eprintln!("available check IDs:");
        let mut ids: Vec<&str> = metas.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        for id in ids {
            eprintln!("  {id}");
        }
    }
    ExitCode::from(2)
}

/// Return up to 5 IDs that look related to `query`. Match strategy is
/// deliberately simple: case-insensitive substring on the dotted ID.
/// Anything fancier (Levenshtein, prefix scoring) is overkill for a
/// catalog of <50 entries.
fn suggestions_for<'a>(
    query: &str,
    metas: &'a [&'static CheckMeta],
) -> Vec<&'a &'static CheckMeta> {
    let needle = query.to_ascii_lowercase();
    let mut matches: Vec<&&CheckMeta> = metas
        .iter()
        .filter(|m| m.id.to_ascii_lowercase().contains(&needle))
        .collect();
    matches.sort_by_key(|m| m.id);
    matches.truncate(5);
    matches
}

#[derive(Serialize)]
struct ExplainReport<'a> {
    id: &'static str,
    category: &'static str,
    default_severity: &'static str,
    base_priority: i8,
    requires_types: bool,
    consistency: bool,
    options: Vec<ExplainOption<'a>>,
    explanation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
}

#[derive(Serialize)]
struct ExplainOption<'a> {
    name: &'static str,
    kind: &'static str,
    default: ExplainDefault<'a>,
    doc: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ExplainDefault<'a> {
    Bool(bool),
    Int(i64),
    String(&'static str),
    StringList(&'a [&'static str]),
    IntList(&'a [i64]),
}

fn build_report(meta: &'static CheckMeta, full: bool) -> ExplainReport<'static> {
    ExplainReport {
        id: meta.id,
        category: category_str(meta.category),
        default_severity: meta.default_severity.as_str(),
        base_priority: meta.base_priority,
        requires_types: meta.requires_types,
        consistency: meta.consistency,
        options: meta.options.iter().map(map_option).collect(),
        explanation: meta.explanation,
        body: if full {
            Some(strip_frontmatter(meta.body).to_string())
        } else {
            None
        },
    }
}

/// Strip the YAML frontmatter block from a companion markdown body.
///
/// The body starts with `---\n…\n---\n`. We locate the second `---` line
/// and return everything after the newline that follows it. If no
/// frontmatter is found the body is returned unchanged.
fn strip_frontmatter(body: &str) -> &str {
    // Find the opening `---`
    if !body.starts_with("---") {
        return body;
    }
    // Skip past the first `---\n`
    let after_open = match body.find('\n') {
        Some(pos) => &body[pos + 1..],
        None => return body,
    };
    // Find the closing `---`
    if let Some(close_pos) = after_open.find("\n---") {
        // Advance past `\n---` and the trailing newline if present
        let rest = &after_open[close_pos + 4..]; // 4 = len("\n---")
        rest.strip_prefix('\n').unwrap_or(rest)
    } else {
        body
    }
}

fn map_option(spec: &'static OptionSpec) -> ExplainOption<'static> {
    let default = match spec.default {
        OptionDefault::Bool(b) => ExplainDefault::Bool(b),
        OptionDefault::Int(i) => ExplainDefault::Int(i),
        OptionDefault::String(s) => ExplainDefault::String(s),
        OptionDefault::StringList(xs) => ExplainDefault::StringList(xs),
        OptionDefault::IntList(xs) => ExplainDefault::IntList(xs),
    };
    ExplainOption {
        name: spec.name,
        kind: spec.kind.name(),
        default,
        doc: spec.doc,
    }
}

fn category_str(cat: Category) -> &'static str {
    match cat {
        Category::Consistency => "consistency",
        Category::Design => "design",
        Category::Readability => "readability",
        Category::Refactor => "refactor",
        Category::Warning => "warning",
    }
}

fn render_text(meta: &'static CheckMeta) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", meta.id);
    let _ = writeln!(out, "  Category:    {}", category_str(meta.category));
    let _ = writeln!(
        out,
        "  Severity:    {} (default)",
        meta.default_severity.as_str()
    );
    let _ = writeln!(out, "  Priority:    {} (base)", meta.base_priority);
    let _ = writeln!(
        out,
        "  Type-aware:  {}",
        if meta.requires_types { "yes" } else { "no" }
    );
    let _ = writeln!(
        out,
        "  Consistency: {}",
        if meta.consistency { "yes" } else { "no" }
    );

    if meta.options.is_empty() {
        let _ = writeln!(out, "  Options:     none");
    } else {
        let _ = writeln!(out, "  Options:");
        for spec in meta.options {
            let _ = writeln!(
                out,
                "    {} ({}, default: {}) — {}",
                spec.name,
                spec.kind.name(),
                format_default(&spec.default),
                spec.doc,
            );
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", meta.explanation);
    out
}

fn format_default(d: &OptionDefault) -> String {
    match d {
        OptionDefault::Bool(b) => b.to_string(),
        OptionDefault::Int(i) => i.to_string(),
        OptionDefault::String(s) => format!("\"{s}\""),
        OptionDefault::StringList(xs) => {
            let inner: Vec<String> = xs.iter().map(|s| format!("\"{s}\"")).collect();
            format!("[{}]", inner.join(", "))
        }
        OptionDefault::IntList(xs) => {
            let inner: Vec<String> = xs.iter().map(i64::to_string).collect();
            format!("[{}]", inner.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{OptionKind, Severity};

    const NO_OPTS_META: CheckMeta = CheckMeta {
        id: "Warning.Test",
        category: Category::Warning,
        base_priority: 15,
        default_severity: Severity::High,
        explanation: "test prose",
        body: "",
        requires_types: false,
        consistency: false,
        options: &[],
    };

    const WITH_OPTS_META: CheckMeta = CheckMeta {
        id: "Readability.MaxLineLength",
        category: Category::Readability,
        base_priority: -5,
        default_severity: Severity::Low,
        explanation: "Lines longer than the configured limit are harder to scan and review.",
        body: "",
        requires_types: false,
        consistency: false,
        options: &[OptionSpec {
            name: "limit",
            kind: OptionKind::Int,
            default: OptionDefault::Int(120),
            doc: "max columns per line",
        }],
    };

    #[test]
    fn render_text_no_options_includes_id_severity_and_prose() {
        let out = render_text(&NO_OPTS_META);
        assert!(out.contains("Warning.Test"));
        assert!(out.contains("Severity:    high"));
        assert!(out.contains("Options:     none"));
        assert!(out.contains("test prose"));
    }

    #[test]
    fn render_text_includes_option_default_and_doc() {
        let out = render_text(&WITH_OPTS_META);
        assert!(out.contains("limit (int, default: 120)"));
        assert!(out.contains("max columns per line"));
    }

    #[test]
    fn build_report_round_trips_metadata() {
        let report = build_report(&WITH_OPTS_META, false);
        let s = serde_json::to_string(&report).expect("valid JSON");
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(parsed["id"], "Readability.MaxLineLength");
        assert_eq!(parsed["category"], "readability");
        assert_eq!(parsed["default_severity"], "low");
        assert_eq!(parsed["base_priority"], -5);
        assert_eq!(parsed["options"][0]["name"], "limit");
        assert_eq!(parsed["options"][0]["default"], 120);
    }

    #[test]
    fn suggestions_substring_match_is_case_insensitive() {
        let metas = vec![&NO_OPTS_META, &WITH_OPTS_META];
        let hits = suggestions_for("max", &metas);
        let ids: Vec<&str> = hits.iter().map(|m| m.id).collect();
        assert!(ids.contains(&"Readability.MaxLineLength"));
        assert!(!ids.contains(&"Warning.Test"));
    }

    #[test]
    fn format_default_renders_string_list_with_quotes() {
        let s = format_default(&OptionDefault::StringList(&["a", "b"]));
        assert_eq!(s, r#"["a", "b"]"#);
    }
}
