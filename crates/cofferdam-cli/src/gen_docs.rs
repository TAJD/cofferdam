//! `cofferdam gen-docs` — regenerate the auto-generated check catalog.
//!
//! Writes (or, with `--check`, asserts no-diff against) six artifact types:
//!
//! - `<out>/public/checks.json`           — schema-stable JSON index (static asset)
//! - `<out>/checks/<id>.md`               — per-check markdown (one per builtin)
//! - `<out>/checks/index.md`              — category-grouped landing page
//! - `<out>/public/llms.txt`              — llmstxt.org root index (static asset)
//! - `<out>/reference/cli.md`             — CLI reference from clap-markdown
//! - `<out>/.vitepress/sidebar-checks.ts` — VitePress sidebar list, imported by config.ts
//!
//! `checks.json` and `llms.txt` go under `<out>/public/` because VitePress
//! only copies files from its `public/` directory to the deployed `dist/`
//! root. Files at the `srcDir` root that aren't `.md` are silently
//! ignored. Net effect: deployed URLs are still
//! `https://<site>/checks.json` and `https://<site>/llms.txt`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Category, CheckMeta, OptionDefault, OptionKind, Severity};
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::Cli;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run(out: PathBuf, check: bool) -> ExitCode {
    // Resolve `out` against CWD so relative paths like "docs" work.
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: could not determine current directory: {e}");
            return ExitCode::FAILURE;
        }
    };
    let base = if out.is_absolute() {
        out
    } else {
        cwd.join(&out)
    };

    // Collect all static metas, sorted by id for determinism.
    let builtins = all_builtins();
    let mut metas: Vec<&'static CheckMeta> = builtins.iter().map(|c| c.meta()).collect();
    metas.sort_by_key(|m| m.id);

    // Build all artifacts as in-memory strings.
    let artifacts = build_all(&metas, &base);

    if check {
        run_check_mode(&artifacts)
    } else {
        run_write_mode(&artifacts)
    }
}

// ---------------------------------------------------------------------------
// Artifact generation
// ---------------------------------------------------------------------------

/// A single generated artifact: absolute path + string content.
struct Artifact {
    path: PathBuf,
    content: String,
}

fn build_all(metas: &[&'static CheckMeta], base: &Path) -> Vec<Artifact> {
    let mut artifacts = Vec::new();

    // public/checks.json — VitePress copies docs/public/* to dist root,
    // so this serves at /<site>/checks.json after deploy.
    artifacts.push(Artifact {
        path: base.join("public").join("checks.json"),
        content: build_checks_json(metas),
    });

    // Per-check markdown
    for meta in metas {
        artifacts.push(Artifact {
            path: base.join("checks").join(format!("{}.md", meta.id)),
            content: build_check_md(meta),
        });
    }

    // checks/index.md
    artifacts.push(Artifact {
        path: base.join("checks").join("index.md"),
        content: build_index_md(metas),
    });

    // public/llms.txt — see public/checks.json comment above.
    artifacts.push(Artifact {
        path: base.join("public").join("llms.txt"),
        content: build_llms_txt(),
    });

    // reference/cli.md
    artifacts.push(Artifact {
        path: base.join("reference").join("cli.md"),
        content: build_cli_md(),
    });

    // .vitepress/sidebar-checks.ts
    artifacts.push(Artifact {
        path: base.join(".vitepress").join("sidebar-checks.ts"),
        content: build_sidebar_checks_ts(metas),
    });

    artifacts
}

// ---------------------------------------------------------------------------
// checks.json
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ChecksJson {
    schema_version: u32,
    generated_by: &'static str,
    cofferdam_version: &'static str,
    checks: Vec<CheckEntry>,
}

#[derive(Serialize)]
struct CheckEntry {
    id: &'static str,
    category: &'static str,
    base_priority: i8,
    default_severity: String,
    explanation: &'static str,
    body: &'static str,
    requires_types: bool,
    consistency: bool,
    options: Vec<OptionEntry>,
}

#[derive(Serialize)]
struct OptionEntry {
    id: &'static str,
    kind: &'static str,
    doc: &'static str,
    default: JsonValue,
}

fn build_checks_json(metas: &[&'static CheckMeta]) -> String {
    let checks: Vec<CheckEntry> = metas
        .iter()
        .map(|m| CheckEntry {
            id: m.id,
            category: category_pascal(m.category),
            base_priority: m.base_priority,
            default_severity: severity_pascal(m.default_severity),
            explanation: m.explanation,
            body: m.body,
            requires_types: m.requires_types,
            consistency: m.consistency,
            options: m.options.iter().map(map_option_entry).collect(),
        })
        .collect();

    let index = ChecksJson {
        schema_version: 1,
        generated_by: "cofferdam gen-docs",
        cofferdam_version: env!("CARGO_PKG_VERSION"),
        checks,
    };

    // Pretty-print with 2-space indent + trailing newline (serde_json default
    // uses 2-space indent in to_string_pretty).
    let mut s = serde_json::to_string_pretty(&index).expect("ChecksJson serializes infallibly");
    s.push('\n');
    s
}

fn map_option_entry(spec: &'static cofferdam_core::OptionSpec) -> OptionEntry {
    OptionEntry {
        id: spec.name,
        kind: option_kind_str(spec.kind),
        doc: spec.doc,
        default: option_default_to_json(&spec.default),
    }
}

fn option_kind_str(kind: OptionKind) -> &'static str {
    match kind {
        OptionKind::Bool => "boolean",
        OptionKind::Int => "integer",
        OptionKind::String => "string",
        OptionKind::StringList => "string[]",
        OptionKind::IntList => "integer[]",
    }
}

fn option_default_to_json(d: &OptionDefault) -> JsonValue {
    match *d {
        OptionDefault::Bool(b) => JsonValue::Bool(b),
        OptionDefault::Int(i) => JsonValue::Number(i.into()),
        OptionDefault::String(s) => JsonValue::String(s.to_string()),
        OptionDefault::StringList(xs) => JsonValue::Array(
            xs.iter()
                .map(|s| JsonValue::String(s.to_string()))
                .collect(),
        ),
        OptionDefault::IntList(xs) => {
            JsonValue::Array(xs.iter().map(|i| JsonValue::Number((*i).into())).collect())
        }
    }
}

// ---------------------------------------------------------------------------
// Per-check markdown
// ---------------------------------------------------------------------------

fn build_check_md(meta: &'static CheckMeta) -> String {
    // Strip the companion file's existing frontmatter; we regenerate it
    // from CheckMeta (the authoritative source of truth).
    let body_without_frontmatter = strip_frontmatter(meta.body);

    // Build the options YAML list for the frontmatter.
    let options_yaml = if meta.options.is_empty() {
        "[]".to_string()
    } else {
        let items: Vec<String> = meta.options.iter().map(|s| s.name.to_string()).collect();
        format!("[{}]", items.join(", "))
    };

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", meta.id));
    out.push_str(&format!("title: {}\n", meta.id));
    out.push_str(&format!("category: {}\n", category_pascal(meta.category)));
    out.push_str(&format!("base_priority: {}\n", meta.base_priority));
    out.push_str(&format!(
        "default_severity: {}\n",
        severity_pascal(meta.default_severity)
    ));
    out.push_str(&format!("options: {}\n", options_yaml));
    out.push_str("---\n");
    out.push('\n');
    out.push_str(
        "<!-- AUTOGENERATED by `cofferdam gen-docs` — edit `crates/cofferdam-checks/docs/",
    );
    out.push_str(meta.id);
    out.push_str(".md` instead -->\n");
    out.push('\n');
    out.push_str(body_without_frontmatter);
    // Ensure trailing newline.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// checks/index.md
// ---------------------------------------------------------------------------

fn build_index_md(metas: &[&'static CheckMeta]) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("title: Built-in checks\n");
    out.push_str("---\n");
    out.push('\n');
    out.push_str("<!-- AUTOGENERATED by `cofferdam gen-docs` — edit `docs/checks/index.md` is overwritten on every run -->\n");
    out.push('\n');
    out.push_str("# Built-in checks\n");
    out.push('\n');
    out.push_str(
        "This catalog is generated from `CheckMeta` in the cofferdam source \
         — every check is guaranteed to be in sync with the running binary. \
         The machine-readable index lives at [`checks.json`](../checks.json) \
         and is consumed by AI agents.\n",
    );

    for cat in Category::ALL {
        let cat_name = category_pascal(cat);
        out.push('\n');
        out.push_str(&format!("## {}\n", cat_name));
        out.push('\n');

        // Checks in this category. `metas` is already globally sorted by id,
        // so the per-category order is sorted-by-id too.
        for meta in metas.iter().filter(|m| m.category == cat) {
            out.push_str(&format!(
                "- [`{}`]({}.md) — {}\n",
                meta.id, meta.id, meta.explanation
            ));
        }
    }

    // Trailing newline.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// llms.txt
// ---------------------------------------------------------------------------

fn build_llms_txt() -> String {
    // URL is locked at https://tajd.github.io/cofferdam per spec (cd-t6n + cd-m77).
    concat!(
        "# Cofferdam\n",
        "\n",
        "> TypeScript code-quality analyzer — Rust core, npm wrapper. Inspired by Elixir's Credo. \
         Five-category model: Consistency, Design, Readability, Refactor, Warning. \
         Priority-sorted output, baseline workflow, CI-friendly.\n",
        "\n",
        "## Install\n",
        "\n",
        "`npm install --save-dev cofferdam`\n",
        "\n",
        "## Catalog\n",
        "\n",
        "- [Machine-readable index](https://tajd.github.io/cofferdam/checks.json): \
         the canonical artifact. JSON, schema_version 1, sorted by check id. \
         AI agents should consume this.\n",
        "- [Human-readable catalog](https://tajd.github.io/cofferdam/checks/): \
         grouped by category, with prose, examples, and configuration.\n",
        "- [CLI reference](https://tajd.github.io/cofferdam/reference/cli): \
         every flag of every subcommand.\n",
        "\n",
        "## Subcommands\n",
        "\n",
        "- `cofferdam check [paths...]`: run all checks. The default workflow.\n",
        "- `cofferdam baseline write|check`: snapshot accepted findings; gate CI on new ones only.\n",
        "- `cofferdam init`: scaffold cofferdam.toml + .cofferdam/baseline.json + .gitignore entries.\n",
        "- `cofferdam explain <id> [--full]`: print metadata + (optionally) prose for one check.\n",
        "- `cofferdam fix [paths...]`: apply autofixes for findings whose check supports it.\n",
        "- `cofferdam doctor [--robot]`: diagnose install/config issues; ✓/⚠/✗ reporting.\n",
        "- `cofferdam gen-docs --out <dir> [--check]`: regenerate this catalog (maintainer-only).\n",
    )
    .to_string()
}

// ---------------------------------------------------------------------------
// reference/cli.md
// ---------------------------------------------------------------------------

fn build_cli_md() -> String {
    let clap_output = clap_markdown::help_markdown::<Cli>();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("title: CLI reference\n");
    out.push_str("---\n");
    out.push('\n');
    out.push_str(
        "<!-- AUTOGENERATED by `cofferdam gen-docs` from clap \
         — edit the clap derive in `crates/cofferdam-cli/src/main.rs` instead -->\n",
    );
    out.push('\n');
    out.push_str(&clap_output);
    // Ensure trailing newline.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// .vitepress/sidebar-checks.ts
// ---------------------------------------------------------------------------

/// Emit the VitePress sidebar list for the Built-in checks group as a TS
/// module. `config.ts` imports `checksItems` from this file. Keeping the
/// list under `gen-docs` means the CI drift gate (`gen-docs --check`)
/// catches any new check that hasn't been wired into the sidebar yet.
fn build_sidebar_checks_ts(metas: &[&'static CheckMeta]) -> String {
    let mut out = String::new();
    out.push_str(
        "// AUTOGENERATED by `cofferdam gen-docs` — do not edit.\n\
         // Source of truth: every entry in `cofferdam_checks::all_builtins()`.\n\
         // Adding a new check? Add it to `all_builtins()`, write its companion\n\
         // file at `crates/cofferdam-checks/docs/<id>.md`, then run `cofferdam gen-docs`.\n\
         //\n\
         // Imported by `config.ts` as the Built-in checks sidebar group.\n\
         \n",
    );
    out.push_str("export const checksItems = [\n");
    out.push_str("  { text: 'All checks', link: '/checks/' },\n");

    for cat in Category::ALL {
        let cat_name = category_pascal(cat);
        // `metas` is globally sorted by id, so per-category order is also sorted.
        let in_cat: Vec<&&'static CheckMeta> = metas.iter().filter(|m| m.category == cat).collect();
        if in_cat.is_empty() {
            continue;
        }
        out.push_str("  {\n");
        out.push_str(&format!("    text: '{}',\n", cat_name));
        out.push_str("    collapsed: true,\n");
        out.push_str("    items: [\n");
        for meta in in_cat {
            // Strip the `Category.` prefix from the leaf entry's `text`;
            // the parent group already labels the category.
            let (_, bare_name) = meta
                .id
                .split_once('.')
                .expect("CheckMeta.id is always `Category.Name`");
            out.push_str(&format!(
                "      {{ text: '{}', link: '/checks/{}' }},\n",
                bare_name, meta.id
            ));
        }
        out.push_str("    ],\n");
        out.push_str("  },\n");
    }

    out.push_str("]\n");
    out
}

// ---------------------------------------------------------------------------
// Write mode
// ---------------------------------------------------------------------------

fn run_write_mode(artifacts: &[Artifact]) -> ExitCode {
    for artifact in artifacts {
        if let Some(parent) = artifact.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("error: could not create {}: {e}", parent.display());
                return ExitCode::FAILURE;
            }
        }
        // Use LF line endings regardless of platform.
        let bytes = normalize_lf(&artifact.content);
        if let Err(e) = std::fs::write(&artifact.path, bytes) {
            eprintln!("error: could not write {}: {e}", artifact.path.display());
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// Check mode
// ---------------------------------------------------------------------------

fn run_check_mode(artifacts: &[Artifact]) -> ExitCode {
    let mut any_stale = false;

    for artifact in artifacts {
        let expected = normalize_lf(&artifact.content);

        let on_disk = match std::fs::read(&artifact.path) {
            Ok(b) => b,
            Err(_) => {
                eprintln!(
                    "out of date: {} (missing — run `cofferdam gen-docs` and commit the result)",
                    artifact.path.display()
                );
                any_stale = true;
                continue;
            }
        };

        // Normalise the on-disk bytes too, so CRLF vs LF doesn't produce
        // phantom diffs when the file was checked out on Windows.
        let on_disk_normalised = normalise_lf_bytes(&on_disk);

        if on_disk_normalised != expected {
            eprintln!(
                "out of date: {} (run `cofferdam gen-docs` and commit the result)",
                artifact.path.display()
            );
            any_stale = true;
        }
    }

    if any_stale {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip the YAML frontmatter block from a companion markdown body.
/// The body starts with `---\n…\n---\n`. Locate the second `---` line
/// and return everything after the newline that follows it. If no
/// frontmatter is found the body is returned unchanged.
fn strip_frontmatter(body: &str) -> &str {
    if !body.starts_with("---") {
        return body;
    }
    let after_open = match body.find('\n') {
        Some(pos) => &body[pos + 1..],
        None => return body,
    };
    if let Some(close_pos) = after_open.find("\n---") {
        let rest = &after_open[close_pos + 4..]; // 4 = len("\n---")
        rest.strip_prefix('\n').unwrap_or(rest)
    } else {
        body
    }
}

/// Convert a string to LF-only bytes. Written artifacts use LF so
/// `git diff` stays clean on Windows.
fn normalize_lf(s: &str) -> Vec<u8> {
    s.replace("\r\n", "\n").into_bytes()
}

/// Same normalisation but on raw bytes (for on-disk comparison in --check mode).
fn normalise_lf_bytes(b: &[u8]) -> Vec<u8> {
    if !b.contains(&b'\r') {
        return b.to_vec();
    }
    let s = String::from_utf8_lossy(b);
    s.replace("\r\n", "\n").into_bytes()
}

fn category_pascal(cat: Category) -> &'static str {
    match cat {
        Category::Consistency => "Consistency",
        Category::Design => "Design",
        Category::Readability => "Readability",
        Category::Refactor => "Refactor",
        Category::Warning => "Warning",
    }
}

fn severity_pascal(sev: Severity) -> String {
    match sev {
        Severity::Info => "Info".to_string(),
        Severity::Low => "Low".to_string(),
        Severity::Medium => "Medium".to_string(),
        Severity::High => "High".to_string(),
        Severity::Critical => "Critical".to_string(),
    }
}
