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

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// `C` is the binary's top-level `clap::Parser` type (`Cli` in `main.rs`).
/// Generic so this library module doesn't depend on the bin crate's type —
/// callers pass their `Cli` struct as the type argument, e.g.
/// `gen_docs::run::<Cli>(out, check)`.
pub fn run<C: clap::CommandFactory>(out: PathBuf, check: bool) -> ExitCode {
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
    let artifacts = build_all::<C>(&metas, &base);

    if check {
        run_check_mode::<C>(&artifacts, &base)
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

fn build_all<C: clap::CommandFactory>(metas: &[&'static CheckMeta], base: &Path) -> Vec<Artifact> {
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
        content: build_cli_md::<C>(),
    });

    // reference/config.md
    artifacts.push(Artifact {
        path: base.join("reference").join("config.md"),
        content: build_config_md(),
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
    autofix: bool,
    options: Vec<OptionEntry>,
    badges: Vec<&'static str>,
}

/// Checks whose findings require cross-file context — the whole-project
/// import/export graph or a duplicate-detection corpus (both built via the
/// corpus + `finalize()`), not just the file in front of them — the
/// "Biome/ESLint structurally can't do this" differentiator (CD-54).
/// Hand-maintained: a new cross-file check should add itself here.
/// `Design.BoundaryFrozen` is deliberately excluded — it matches a
/// per-file glob against `[boundaries]`, no cross-file context needed.
const GRAPH_CHECK_IDS: &[&str] = &[
    "Design.ImportCycle",
    "Design.InvariantViolation",
    "Design.LayerViolation",
    "Design.OrphanExport",
    "Design.ScriptedInvariant",
    "Design.DuplicateExportName",
    "Refactor.DeadExport",
    "Refactor.DuplicateBlock",
];

/// Checks `advise` gives file-specific, non-generic treatment to today
/// (CD-62 WS-A1-A3): every option-bearing check resolves its thresholds,
/// plus these graph-aware checks project layer/boundary/invariant state
/// even though they take no options.
const ADVISABLE_SPECIAL_CASE_IDS: &[&str] = &[
    "Design.LayerViolation",
    "Design.BoundaryFrozen",
    "Design.InvariantViolation",
    "Design.OrphanExport",
];

/// Badges for the checks catalog (CD-54): `graph` / `file` (mutually
/// exclusive — whole-project graph vs. per-file analysis), `type-aware`
/// (routes through the ts-morph type host), `advisable` (`cofferdam
/// advise` emits a file-specific, non-generic constraint for it).
fn check_badges(meta: &'static CheckMeta) -> Vec<&'static str> {
    let mut badges = Vec::new();
    if GRAPH_CHECK_IDS.contains(&meta.id) {
        badges.push("graph");
    } else {
        badges.push("file");
    }
    if meta.requires_types {
        badges.push("type-aware");
    }
    if !meta.options.is_empty() || ADVISABLE_SPECIAL_CASE_IDS.contains(&meta.id) {
        badges.push("advisable");
    }
    badges
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
            autofix: m.autofix,
            options: m.options.iter().map(map_option_entry).collect(),
            badges: check_badges(m),
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
    out.push_str(&format!("autofix: {}\n", meta.autofix));
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
    out.push('\n');
    out.push_str(
        "**Badges:** `graph` — needs cross-file context: the whole-project \
         import/export graph or a duplicate-detection corpus (what Biome/ESLint \
         structurally can't do); `file` — analyzes one \
         file in isolation; `type-aware` — routes through the ts-morph type \
         host; `advisable` — `cofferdam advise` emits a file-specific \
         constraint for it (not just the generic explanation).\n",
    );

    for cat in Category::ALL {
        let cat_name = category_pascal(cat);
        out.push('\n');
        out.push_str(&format!("## {}\n", cat_name));
        out.push('\n');

        // Checks in this category. `metas` is already globally sorted by id,
        // so the per-category order is sorted-by-id too.
        for meta in metas.iter().filter(|m| m.category == cat) {
            let autofix_badge = if meta.autofix { " · autofix" } else { "" };
            let badges = check_badges(meta)
                .into_iter()
                .map(|b| format!("`{b}`"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!(
                "- [`{}`]({}.md) {} — {}{}\n",
                meta.id, meta.id, badges, meta.explanation, autofix_badge
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
    // The `Current version:` line is audited by `scripts/version.mjs check`
    // (cd-54sh) — keep its exact shape if you edit this.
    format!(
        concat!(
            "# Cofferdam\n",
            "\n",
            "> TypeScript code-quality analyzer — Rust core, npm wrapper. Inspired by Elixir's Credo. \
             Five-category model: Consistency, Design, Readability, Refactor, Warning. \
             Priority-sorted output, baseline workflow, CI-friendly. \
             Current version: {version}.\n",
            "\n",
            "## Install\n",
            "\n",
            "`npm install --save-dev @cofferdam/cofferdam`\n",
            "\n",
            "Prebuilt binaries download on postinstall (Linux x64/arm64 glibc+musl, macOS x64/arm64, \
             Windows x64; Node 16+). Escape hatches and air-gapped installs: \
             [install guide](https://tajd.github.io/cofferdam/install).\n",
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
            "- [Full page content](https://tajd.github.io/cofferdam/llms-full.txt): \
             every docs page's raw markdown, concatenated — one fetch instead of one per page.\n",
            "\n",
            "## Docs\n",
            "\n",
            "- [Agent advisory](https://tajd.github.io/cofferdam/reference/advise): \
             `cofferdam advise` — the rules that apply to a file, before you edit it.\n",
            "- [Output formats](https://tajd.github.io/cofferdam/output-formats): \
             text, JSON, compact, SARIF.\n",
            "- [CI recipes](https://tajd.github.io/cofferdam/ci-recipes): \
             GitHub Actions, GitLab, CircleCI, Drone, pre-commit.\n",
            "- [Suppression directives](https://tajd.github.io/cofferdam/suppression): \
             `// cofferdam-ignore` syntax.\n",
            "- [Ignoring files](https://tajd.github.io/cofferdam/ignore): \
             `.cofferdamignore` rules.\n",
            "- [Per-path overrides](https://tajd.github.io/cofferdam/overrides): \
             retune or disable single checks on matching globs.\n",
            "- [Type-aware checks](https://tajd.github.io/cofferdam/type-aware-checks): \
             checks backed by the TypeScript type system; requirements and opt-out.\n",
            "- [Architectural invariants](https://tajd.github.io/cofferdam/invariants): \
             `cofferdam.invariants.toml`, layering rules, frozen boundaries.\n",
            "- [Plugin SDK](https://tajd.github.io/cofferdam/plugin-sdk-guide): \
             write custom checks in TypeScript with `@cofferdam/check-sdk`.\n",
            "- [Doctor](https://tajd.github.io/cofferdam/doctor): \
             diagnose install/config issues.\n",
            "\n",
            "## Subcommands\n",
            "\n",
            "- `cofferdam check [paths...]`: run all checks. The default workflow. \
             `--robot` switches to token-economical machine-readable output.\n",
            "- `cofferdam verify --dist <dir>`: opt-in check mode for built HTML output — runs \
             only output-mode-tagged checks against a `--dist` tree, separate from `check`.\n",
            "- `cofferdam agents`: print a version-pinned onboarding prompt for AI coding agents \
             — pipe into AGENTS.md / CLAUDE.md.\n",
            "- `cofferdam advise <file>`: print the rules and constraints that apply to a file \
             — run this BEFORE editing.\n",
            "- `cofferdam advise --diff <git-ref>`: pre-flight a proposed change; reports \
             `would_fire` / `would_clear` against the working tree.\n",
            "- `cofferdam context [paths...]`: post-edit context digest — delta-scoped \
             findings, blast radius, precedent, and curated knowledge for the current diff.\n",
            "- `cofferdam baseline write`: snapshot accepted findings so future `check` runs \
             ignore them.\n",
            "- `cofferdam baseline lint`: gate CI on findings not already in the baseline.\n",
            "- `cofferdam baseline diff`: compare two baselines (e.g. before/after a refactor).\n",
            "- `cofferdam init`: scaffold cofferdam.toml + .cofferdam/baseline.json + .gitignore entries.\n",
            "- `cofferdam explain <id> [--full]`: print metadata + (optionally) prose for one check.\n",
            "- `cofferdam fix [paths...]`: apply autofixes for findings whose check supports it.\n",
            "- `cofferdam watch [paths...]`: re-run checks on file change.\n",
            "- `cofferdam doctor [--robot]`: diagnose install/config issues; ✓/⚠/✗ reporting.\n",
            "- `cofferdam lsp`: LSP server over stdio for editor integration.\n",
            "- `cofferdam typst <dir>`: lint a Typst package directory.\n",
            "- `cofferdam gen-docs --out <dir> [--check]`: regenerate this catalog (maintainer-only).\n",
            "- `cofferdam hello`: print the project banner.\n",
            "\n",
            "## Agent workflow\n",
            "\n",
            "Run `cofferdam agents` for the full onboarding prompt. The short version:\n",
            "\n",
            "1. `cofferdam advise <file>` before editing — learn the layering, complexity, and \
             style constraints that apply to that file.\n",
            "2. Make the change.\n",
            "3. `cofferdam advise --diff <base-ref>` to pre-flight — `would_fire` should be empty \
             or justified.\n",
            "4. `cofferdam check --robot` for machine-readable findings; fix them, or suppress \
             with a reasoned `// cofferdam-ignore: <CheckId>: <reason>`.\n",
            "\n",
            "`cofferdam.invariants.toml` at the repo root is the source of architectural truth — \
             read it before structural changes.\n",
        ),
        version = env!("CARGO_PKG_VERSION"),
    )
}

// ---------------------------------------------------------------------------
// reference/cli.md
// ---------------------------------------------------------------------------

fn build_cli_md<C: clap::CommandFactory>() -> String {
    let clap_output = clap_markdown::help_markdown::<C>();
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
// reference/config.md
// ---------------------------------------------------------------------------

/// Render the `cofferdam.toml` key surface from
/// `cofferdam_engine::config::schema` (CD-311).
///
/// The same table drives the unknown-key warning, so a key cannot be
/// recognised by the loader and missing from this page, or vice versa.
/// That is the whole point: the CLI reference is generated from clap and
/// therefore cannot drift, and config had no equivalent — which is where
/// three of the docs sweep's worst findings came from, including an
/// `exclude` key that two pages promised and no code has ever read.
fn build_config_md() -> String {
    use cofferdam_engine::config::{Keys, SECTIONS};

    let mut out = String::new();
    out.push_str("---\ntitle: Config reference\n---\n\n");
    out.push_str(
        "<!-- AUTOGENERATED by `cofferdam gen-docs` from the schema in \
         `crates/cofferdam-engine/src/config/schema.rs` — edit that instead -->\n\n",
    );
    out.push_str("# `cofferdam.toml` reference\n\n");
    out.push_str(
        "Every key cofferdam reads. Generated from the loader's own schema, so it \
         cannot fall out of step with what the binary accepts.\n\n\
         A key that appears in no section below is ignored. Cofferdam says so when it \
         loads the file — an unknown key used to parse cleanly and do nothing, which \
         made a typo indistinguishable from a rule that simply never matched.\n\n\
         Exclusion is not configured here: it is a discovery-time decision and lives in \
         [`.cofferdamignore`](/ignore) or `.gitignore`. To turn a check off over a path \
         glob without removing the file from analysis, use `[[overrides]]` with \
         `disabled = true`.\n\n",
    );

    for section in SECTIONS {
        out.push_str(&format!("## {}\n\n", section.title));
        out.push_str(section.doc);
        out.push_str("\n\n");

        let keys = match section.keys {
            Keys::Fixed(k) => k,
            Keys::Open {
                key_meaning,
                entry_keys,
            } => {
                out.push_str(&format!(
                    "Keys are chosen by you — each is a {key_meaning}.\n\n"
                ));
                entry_keys
            }
        };
        if keys.is_empty() {
            continue;
        }

        out.push_str("| Key | Type | Default | Meaning |\n|---|---|---|---|\n");
        for k in keys {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                k.key,
                k.type_name,
                k.default.unwrap_or("—"),
                k.doc.replace('|', "\\|"),
            ));
        }
        out.push('\n');
    }

    out.push_str(
        "## Related files\n\n\
         - [`cofferdam.invariants.toml`](/invariants) — layers, public API, frozen \
         boundaries and scripted invariants. Run `cofferdam invariants show` to see what \
         a run actually resolved.\n\
         - [`.cofferdamignore`](/ignore) — files to keep out of analysis entirely.\n\
         - [`.cofferdam/knowledge/*.md`](/knowledge-notes) — curated notes surfaced by \
         `cofferdam context`.\n",
    );
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

fn run_check_mode<C: clap::CommandFactory>(artifacts: &[Artifact], base: &Path) -> ExitCode {
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

    // Drift gate: every non-hidden top-level subcommand must be
    // mentioned in llms.txt (CD-63 E3). Checked against the generated
    // content directly so a new Cmd variant with no llms.txt update
    // fails --check even before the artifact is regenerated on disk.
    if let Some(llms) = artifacts.iter().find(|a| a.path.ends_with("llms.txt")) {
        let missing = missing_subcommands_in_llms_txt::<C>(&llms.content);
        if !missing.is_empty() {
            eprintln!(
                "drift: llms.txt is missing subcommand(s): {} (update build_llms_txt in gen_docs.rs)",
                missing.join(", ")
            );
            any_stale = true;
        }
    }

    // Denylist gate: hand-authored docs must not claim something
    // "doesn't exist yet" — mcp.md said exactly that about advise/
    // advise_diff/explain/invariants for months after they shipped
    // (CD-63 E3, seeded with that incident).
    for (path, phrase) in find_stale_claims(base) {
        eprintln!(
            "stale claim: {} contains {phrase:?} — verify the claim and rephrase or remove it",
            path.display()
        );
        any_stale = true;
    }

    // Nav-completeness gate (CD-111): a real doc page nobody links to is
    // invisible on the deployed site. Six pages went unlinked for months
    // before CD-109 caught it by hand (CD-113).
    for (path, reason) in find_orphaned_nav_pages(base) {
        eprintln!(
            "orphaned page: {} — {reason}. Add it to the sidebar in docs/.vitepress/config.ts (or NAV_ORPHAN_ALLOWLIST if intentional).",
            path.display()
        );
        any_stale = true;
    }

    if any_stale {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Every non-hidden top-level subcommand's name (e.g. `"gen-docs"`) must
/// appear as `cofferdam <name>` somewhere in llms.txt — otherwise an agent
/// skimming llms.txt has no way to discover the subcommand exists.
fn missing_subcommands_in_llms_txt<C: clap::CommandFactory>(llms_txt: &str) -> Vec<String> {
    C::command()
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| c.get_name().to_string())
        .filter(|name| !llms_txt.contains(&format!("cofferdam {name}")))
        .collect()
}

/// Forward-looking phrases that describe something as not-yet-existing.
/// Seeded with the exact phrase from the mcp.md incident (CD-60): docs
/// claimed the advise/advise_diff/explain/invariants MCP tools "depend on
/// features that don't exist yet" for months after those features
/// shipped. A denylist can't know when a true claim goes stale on its
/// own — but it stops a *new* stale claim from being added, and forces a
/// human decision (rephrase or remove) the moment CI catches a match.
const STALE_CLAIM_DENYLIST: &[&str] = &[
    "doesn't exist yet",
    "don't exist yet",
    "coming soon",
    "not yet implemented",
    "not yet available",
];

/// Recursively scan hand-authored `.md` files under `base` (skipping
/// generated/vendor directories) for `STALE_CLAIM_DENYLIST` phrases.
/// Returns one `(path, phrase)` entry per match.
fn find_stale_claims(base: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut hits = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // `superpowers/` holds plan/spec scratch docs, not VitePress
                // site content (not referenced from config.ts nav) — plans
                // legitimately describe unimplemented future work.
                if matches!(
                    name,
                    ".vitepress" | "node_modules" | "dist" | "public" | "superpowers"
                ) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let lower = content.to_lowercase();
            for phrase in STALE_CLAIM_DENYLIST {
                if lower.contains(phrase) {
                    hits.push((path.clone(), *phrase));
                }
            }
        }
    }
    hits
}

/// Pages intentionally absent from the site nav. `README.md` is VitePress
/// build instructions for contributors, not a published page.
const NAV_ORPHAN_ALLOWLIST: &[&str] = &["/README"];

/// Every hand-authored `docs/**/*.md` page must be reachable from the
/// VitePress nav. Walk the tree (same skip-list as `find_stale_claims`),
/// resolve each file to the site link VitePress serves it at, and flag any
/// link absent from `config.ts` (plus the generated `sidebar-checks.ts`,
/// which `config.ts` imports). Returns one `(path, reason)` per orphan.
fn find_orphaned_nav_pages(base: &Path) -> Vec<(PathBuf, String)> {
    let vp = base.join(".vitepress");
    let mut nav_src = std::fs::read_to_string(vp.join("config.ts")).unwrap_or_default();
    if let Ok(sidebar) = std::fs::read_to_string(vp.join("sidebar-checks.ts")) {
        nav_src.push_str(&sidebar);
    }
    if nav_src.is_empty() {
        return Vec::new();
    }

    let mut orphans = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    ".vitepress" | "node_modules" | "dist" | "public" | "superpowers"
                ) {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let link = site_link(base, &path);
            if NAV_ORPHAN_ALLOWLIST.contains(&link.as_str()) {
                continue;
            }
            if !link_in_config(&nav_src, &link) {
                orphans.push((path.clone(), format!("no nav entry for {link}")));
            }
        }
    }
    orphans
}

/// Resolve a `docs/**/*.md` path to the site-relative link VitePress serves
/// it at: strip the `base` prefix and `.md` suffix, and collapse `index` to
/// a trailing slash (`checks/index.md` → `/checks/`, root → `/`).
fn site_link(base: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    let s = s.strip_suffix(".md").unwrap_or(&s);
    if s == "index" {
        return "/".to_string();
    }
    if let Some(dir) = s.strip_suffix("/index") {
        return format!("/{dir}/");
    }
    format!("/{s}")
}

/// True if `link` (or a trailing-slash variant) appears as a quoted value in
/// the raw nav source. Tolerant of `'…'` vs `"…"` and a trailing slash.
fn link_in_config(nav_src: &str, link: &str) -> bool {
    let mut variants = vec![link.to_string()];
    if link != "/" {
        let trimmed = link.trim_end_matches('/');
        variants.push(trimmed.to_string());
        variants.push(format!("{trimmed}/"));
    }
    variants
        .iter()
        .any(|v| nav_src.contains(&format!("'{v}'")) || nav_src.contains(&format!("\"{v}\"")))
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
        Category::Context => "Context",
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

#[cfg(test)]
mod tests {
    use super::find_orphaned_nav_pages;

    #[test]
    fn flags_orphan_but_not_wired_or_allowlisted() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        std::fs::create_dir_all(base.join(".vitepress")).unwrap();
        std::fs::write(
            base.join(".vitepress").join("config.ts"),
            "sidebar: [{ text: 'X', link: '/wired' }]",
        )
        .unwrap();
        std::fs::write(base.join("wired.md"), "# wired").unwrap();
        std::fs::write(base.join("orphan.md"), "# orphan").unwrap();
        std::fs::write(base.join("README.md"), "# build steps").unwrap();

        let names: Vec<String> = find_orphaned_nav_pages(base)
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["orphan.md".to_string()]);
    }
}
