//! `cofferdam explain <check_id>` — surface a check's metadata and
//! prose explanation in the terminal.
//!
//! With `--full`, also prints the companion markdown body from
//! `CheckMeta::body` (extracted from `crates/cofferdam-checks/docs/`).
//! The frontmatter block (`---…---`) is stripped before terminal output.
//! In `--robot` mode the body is included as a `body` JSON field.
//!
//! When the requested ID is not found in the built-in catalog, the
//! subcommand resolves `cofferdam.toml` and queries any declared plugins
//! via the plugin host's metadata mode. Plugin checks are rendered the
//! same way as built-ins (human + robot output paths).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Category, CheckMeta, OptionDefault, OptionSpec};
use cofferdam_engine::config::{self as cfg};
use serde::Serialize;

use crate::plugins::{self, PluginCheckMeta};

pub struct ExplainArgs {
    pub check_id: String,
    pub robot: bool,
    pub pretty: bool,
    pub full: bool,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
}

pub fn run(args: ExplainArgs) -> ExitCode {
    let ExplainArgs {
        check_id,
        robot,
        pretty,
        full,
        config_path,
        no_config,
    } = args;

    // `Vec<&'static CheckMeta>`: cheap, the metas are static.
    let metas: Vec<&'static CheckMeta> = all_builtins().iter().map(|c| c.meta()).collect();

    if let Some(meta) = metas.iter().find(|m| m.id == check_id).copied() {
        return render_builtin(meta, robot, pretty, full);
    }

    // Not in built-ins — try plugin-declared checks from cofferdam.toml.
    let (project_config, config_path_resolved) =
        match resolve_and_load_config(config_path.as_deref(), no_config) {
            Ok(pair) => pair,
            Err(()) => return ExitCode::from(2),
        };

    let plugin_metas = if let Some(cfg) = project_config.as_ref() {
        if !cfg.plugins.is_empty() {
            let cfg_dir = config_path_resolved
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            plugins::query_plugin_metadata(&cfg.plugins, &cfg_dir)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    if let Some(pm) = plugin_metas.iter().find(|pm| pm.id == check_id) {
        return render_plugin(pm, robot, pretty, full);
    }

    not_found(&check_id, &metas, &plugin_metas, robot)
}

fn render_builtin(meta: &'static CheckMeta, robot: bool, pretty: bool, full: bool) -> ExitCode {
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

fn render_plugin(pm: &PluginCheckMeta, robot: bool, pretty: bool, full: bool) -> ExitCode {
    if robot {
        let report = build_plugin_report(pm, full);
        let s = if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
        .expect("PluginExplainReport serializes infallibly");
        println!("{}", s);
    } else {
        print!("{}", render_plugin_text(pm));
        if full {
            match &pm.body {
                Some(body) if !body.is_empty() => {
                    println!("\n---\n");
                    print!("{body}");
                }
                _ => {
                    println!();
                    println!("  (this plugin check does not ship a long-form body)");
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// Print "no such check" plus a short shortlist of close-ish IDs (or
/// the full list if no near match). Exit 2 — same convention used
/// elsewhere in the CLI for usage / "input was wrong" failures.
fn not_found(
    query: &str,
    metas: &[&'static CheckMeta],
    plugin_metas: &[PluginCheckMeta],
    robot: bool,
) -> ExitCode {
    if robot {
        // Schema-stable error shape: `{ "error": "...", "suggestions": [...] }`.
        // Agents can branch on `error` without parsing prose.
        #[derive(Serialize)]
        struct ErrReport {
            error: String,
            suggestions: Vec<String>,
        }
        let suggestions = suggestions_for(query, metas, plugin_metas);
        let report = ErrReport {
            error: format!("no such check: {query}"),
            suggestions,
        };
        let s = serde_json::to_string(&report).expect("ErrReport serializes infallibly");
        // Errors go to stderr; the `--robot` JSON contract is stdout-
        // for-success-only, and this is a usage error.
        eprintln!("{}", s);
        return ExitCode::from(2);
    }

    eprintln!("error: no such check: {query}");
    let suggestions = suggestions_for(query, metas, plugin_metas);
    if !suggestions.is_empty() {
        eprintln!();
        eprintln!("did you mean:");
        for id in &suggestions {
            eprintln!("  {id}");
        }
    } else {
        eprintln!();
        eprintln!("available check IDs:");
        let mut ids: Vec<String> = metas.iter().map(|m| m.id.to_string()).collect();
        ids.extend(plugin_metas.iter().map(|pm| pm.id.clone()));
        ids.sort_unstable();
        for id in ids {
            eprintln!("  {id}");
        }
    }
    ExitCode::from(2)
}

/// Return up to 5 IDs that look related to `query` — drawn from both the
/// built-in catalog and any loaded plugin checks. Match strategy is
/// deliberately simple: case-insensitive substring on the dotted ID.
fn suggestions_for(
    query: &str,
    metas: &[&'static CheckMeta],
    plugin_metas: &[PluginCheckMeta],
) -> Vec<String> {
    let needle = query.to_ascii_lowercase();
    let mut matches: Vec<String> = metas
        .iter()
        .filter(|m| m.id.to_ascii_lowercase().contains(&needle))
        .map(|m| m.id.to_string())
        .chain(
            plugin_metas
                .iter()
                .filter(|pm| pm.id.to_ascii_lowercase().contains(&needle))
                .map(|pm| pm.id.clone()),
        )
        .collect();
    matches.sort_unstable();
    matches.dedup();
    matches.truncate(5);
    matches
}

#[derive(Serialize)]
pub struct ExplainReport<'a> {
    pub id: &'static str,
    pub category: &'static str,
    pub default_severity: &'static str,
    pub base_priority: i8,
    pub requires_types: bool,
    pub consistency: bool,
    pub autofix: bool,
    pub options: Vec<ExplainOption<'a>>,
    pub explanation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize)]
pub struct ExplainOption<'a> {
    pub name: &'static str,
    pub kind: &'static str,
    pub default: ExplainDefault<'a>,
    pub doc: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ExplainDefault<'a> {
    Bool(bool),
    Int(i64),
    String(&'static str),
    StringList(&'a [&'static str]),
    IntList(&'a [i64]),
}

/// Outcome of resolving a check ID to its explanation. Shared by the
/// CLI's `run()` and `cofferdam-mcp`'s `cofferdam.explain` tool (CD-60).
pub enum ExplainOutcome {
    Builtin(ExplainReport<'static>),
    Plugin(PluginExplainReport),
    NotFound {
        error: String,
        suggestions: Vec<String>,
    },
}

/// Library entry point: resolve `check_id` against built-ins, then
/// against any plugins declared in the resolved config. Pure — does no
/// printing or process exit.
pub fn explain_check(
    check_id: &str,
    config_path: Option<&Path>,
    no_config: bool,
    full: bool,
) -> ExplainOutcome {
    let metas: Vec<&'static CheckMeta> = all_builtins().iter().map(|c| c.meta()).collect();

    if let Some(meta) = metas.iter().find(|m| m.id == check_id).copied() {
        return ExplainOutcome::Builtin(build_report(meta, full));
    }

    let (project_config, config_path_resolved) =
        match resolve_and_load_config(config_path, no_config) {
            Ok(pair) => pair,
            Err(()) => {
                return ExplainOutcome::NotFound {
                    error: format!("no such check: {check_id}"),
                    suggestions: Vec::new(),
                }
            }
        };

    let plugin_metas = if let Some(cfg) = project_config.as_ref() {
        if !cfg.plugins.is_empty() {
            let cfg_dir = config_path_resolved
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            plugins::query_plugin_metadata(&cfg.plugins, &cfg_dir)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    if let Some(pm) = plugin_metas.iter().find(|pm| pm.id == check_id) {
        return ExplainOutcome::Plugin(build_plugin_report(pm, full));
    }

    ExplainOutcome::NotFound {
        error: format!("no such check: {check_id}"),
        suggestions: suggestions_for(check_id, &metas, &plugin_metas),
    }
}

fn build_report(meta: &'static CheckMeta, full: bool) -> ExplainReport<'static> {
    ExplainReport {
        id: meta.id,
        category: category_str(meta.category),
        default_severity: meta.default_severity.as_str(),
        base_priority: meta.base_priority,
        requires_types: meta.requires_types,
        consistency: meta.consistency,
        autofix: meta.autofix,
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
        Category::Context => "context",
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
    let _ = writeln!(
        out,
        "  Autofix:     {}",
        if meta.autofix { "yes" } else { "no" }
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

// ---- plugin check rendering ----

#[derive(Serialize)]
pub struct PluginExplainReport {
    pub id: String,
    pub category: String,
    pub default_severity: String,
    pub base_priority: i64,
    pub requires_types: bool,
    pub options: Vec<PluginExplainOption>,
    pub explanation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize)]
pub struct PluginExplainOption {
    pub name: String,
    pub kind: String,
    pub default: serde_json::Value,
    pub doc: String,
}

fn build_plugin_report(pm: &PluginCheckMeta, full: bool) -> PluginExplainReport {
    PluginExplainReport {
        id: pm.id.clone(),
        category: pm.category.clone(),
        default_severity: pm.default_severity.clone(),
        base_priority: pm.base_priority,
        requires_types: pm.requires_types,
        options: pm
            .options
            .iter()
            .map(|o| PluginExplainOption {
                name: o.name.clone(),
                kind: o.kind.clone(),
                default: o.default.clone(),
                doc: o.doc.clone(),
            })
            .collect(),
        explanation: pm.explanation.clone(),
        body: if full { pm.body.clone() } else { None },
    }
}

fn render_plugin_text(pm: &PluginCheckMeta) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} (plugin)", pm.id);
    let _ = writeln!(out, "  Category:    {}", pm.category);
    let _ = writeln!(out, "  Severity:    {} (default)", pm.default_severity);
    let _ = writeln!(out, "  Priority:    {} (base)", pm.base_priority);
    let _ = writeln!(
        out,
        "  Type-aware:  {}",
        if pm.requires_types { "yes" } else { "no" }
    );

    if pm.options.is_empty() {
        let _ = writeln!(out, "  Options:     none");
    } else {
        let _ = writeln!(out, "  Options:");
        for opt in &pm.options {
            let _ = writeln!(
                out,
                "    {} ({}, default: {}) — {}",
                opt.name, opt.kind, opt.default, opt.doc,
            );
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", pm.explanation);
    out
}

/// Resolve the `cofferdam.toml` config from the given explicit path or by
/// walking up from CWD. Mirrors the same helper in `main.rs`.
fn resolve_and_load_config(
    explicit: Option<&Path>,
    no_config: bool,
) -> Result<(Option<cofferdam_engine::ProjectConfig>, Option<PathBuf>), ()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match cfg::resolve_with_invariants(explicit, &cwd, no_config) {
        Ok((config, path, diags)) => {
            for w in &diags.warnings {
                eprintln!("warning: {w}");
            }
            Ok((config, path))
        }
        Err(e) => {
            eprintln!("error: {e}");
            Err(())
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
        autofix: false,
        pure_run: false,
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
        autofix: false,
        pure_run: false,
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
    fn render_text_autofix_no_shows_for_non_autofix_check() {
        let out = render_text(&WITH_OPTS_META);
        // WITH_OPTS_META has autofix: false
        assert!(
            out.contains("Autofix:     no"),
            "expected 'Autofix:     no' in:\n{out}"
        );
    }

    #[test]
    fn render_text_autofix_yes_shows_for_autofix_check() {
        // Build a meta with autofix: true to verify the yes branch.
        const AUTOFIX_META: CheckMeta = CheckMeta {
            id: "Warning.TripleEqualsTest",
            category: Category::Warning,
            base_priority: 15,
            default_severity: Severity::High,
            explanation: "test",
            body: "",
            requires_types: false,
            consistency: false,
            options: &[],
            autofix: true,
            pure_run: false,
        };
        let out = render_text(&AUTOFIX_META);
        assert!(
            out.contains("Autofix:     yes"),
            "expected 'Autofix:     yes' in:\n{out}"
        );
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
    fn build_report_includes_autofix_field() {
        let report = build_report(&NO_OPTS_META, false);
        let s = serde_json::to_string(&report).expect("valid JSON");
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        // NO_OPTS_META has autofix: false
        assert_eq!(parsed["autofix"], false);
    }

    #[test]
    fn explain_triple_equals_autofix_true() {
        use cofferdam_checks::all_builtins;
        let builtins = all_builtins();
        let metas: Vec<&'static CheckMeta> = builtins.iter().map(|c| c.meta()).collect();
        let meta = metas
            .iter()
            .find(|m| m.id == "Warning.TripleEquals")
            .copied()
            .expect("Warning.TripleEquals must exist");
        let out = render_text(meta);
        assert!(
            out.contains("Autofix:     yes"),
            "expected yes for Warning.TripleEquals:\n{out}"
        );
        let report = build_report(meta, false);
        let s = serde_json::to_string(&report).expect("valid JSON");
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(parsed["autofix"], true);
    }

    #[test]
    fn explain_max_line_length_autofix_false() {
        use cofferdam_checks::all_builtins;
        let builtins = all_builtins();
        let metas: Vec<&'static CheckMeta> = builtins.iter().map(|c| c.meta()).collect();
        let meta = metas
            .iter()
            .find(|m| m.id == "Readability.MaxLineLength")
            .copied()
            .expect("Readability.MaxLineLength must exist");
        let out = render_text(meta);
        assert!(
            out.contains("Autofix:     no"),
            "expected no for Readability.MaxLineLength:\n{out}"
        );
        let report = build_report(meta, false);
        let s = serde_json::to_string(&report).expect("valid JSON");
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(parsed["autofix"], false);
    }

    #[test]
    fn suggestions_substring_match_is_case_insensitive() {
        let metas = vec![&NO_OPTS_META, &WITH_OPTS_META];
        let hits = suggestions_for("max", &metas, &[]);
        assert!(hits.iter().any(|id| id == "Readability.MaxLineLength"));
        assert!(!hits.iter().any(|id| id == "Warning.Test"));
    }

    #[test]
    fn suggestions_includes_plugin_ids() {
        let metas = vec![&NO_OPTS_META];
        let plugin_metas = vec![crate::plugins::PluginCheckMeta {
            path: "./plugin".to_string(),
            id: "Warning.MaxFoo".to_string(),
            category: "warning".to_string(),
            base_priority: 5,
            explanation: "foo check".to_string(),
            default_severity: "medium".to_string(),
            body: None,
            requires_types: false,
            output_mode: false,
            options: Vec::new(),
            files: None,
        }];
        let hits = suggestions_for("foo", &metas, &plugin_metas);
        assert!(hits.iter().any(|id| id == "Warning.MaxFoo"));
    }

    #[test]
    fn format_default_renders_string_list_with_quotes() {
        let s = format_default(&OptionDefault::StringList(&["a", "b"]));
        assert_eq!(s, r#"["a", "b"]"#);
    }
}
