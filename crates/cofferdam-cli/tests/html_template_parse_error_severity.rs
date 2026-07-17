//! Regression test for CD-101: `Warning.ParseError` from the HTML adapter's
//! recovered-parse path (tree-sitter-html hit ERROR/MISSING nodes) fired at
//! `Critical` severity on legitimate ERB/EJS `<% %>` scriptlets and Jinja
//! nested-double-quote attribute idioms — template-source constructs that
//! are only invalid as raw, unrendered HTML. Downgraded to `Low` so it
//! surfaces for visibility without failing CI at the default
//! `--fail-on=medium`.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn parse_error_findings(root: &std::path::Path, filename: &str) -> serde_json::Value {
    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline", "--format=json", filename])
        .current_dir(root)
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "cofferdam stdout not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    v
}

fn assert_low_severity_parse_error(v: &serde_json::Value, label: &str) {
    let findings = v["findings"].as_array().expect("findings array");
    let parse_errors: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|f| f["id"].as_str() == Some("Warning.ParseError"))
        .collect();
    assert!(
        !parse_errors.is_empty(),
        "{label}: expected a Warning.ParseError finding (tree-sitter-html has no grammar \
         for this template idiom); findings={findings:?}"
    );
    for f in &parse_errors {
        assert_eq!(
            f["severity"].as_str(),
            Some("low"),
            "{label}: HTML adapter recovered-parse Warning.ParseError must be Low severity \
             (template-source false alarm, not a real bug); got {f:?}"
        );
    }
}

/// Root cause 1 (CD-101): ERB/EJS `<% %>` scriptlet — the leading `<` in
/// `<%` is indistinguishable from a malformed tag-open to an HTML-only
/// grammar.
#[test]
fn erb_style_scriptlet_reports_low_severity_parse_error() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("index.html"),
        "<html><body><% if (user) { %><p>Hi <%= user.name %></p><% } %></body></html>\n",
    )
    .expect("write html");

    let v = parse_error_findings(dir.path(), "index.html");
    assert_low_severity_parse_error(&v, "ERB/EJS scriptlet");
}

/// Root cause 2 (CD-101): Jinja nested-double-quote attribute idiom, e.g.
/// `action="{{ url_for("tasks.add") }}"` — genuinely invalid standalone
/// HTML (unescaped `"` inside a double-quoted attribute value), only valid
/// after Jinja renders it away.
#[test]
fn jinja_nested_quote_attribute_reports_low_severity_parse_error() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("index.html"),
        "<html><body><form action=\"{{ url_for(\"tasks.add\") }}\"></form></body></html>\n",
    )
    .expect("write html");

    let v = parse_error_findings(dir.path(), "index.html");
    assert_low_severity_parse_error(&v, "Jinja nested-quote attribute");
}

/// A clean, fully-rendered HTML file must never trip `Warning.ParseError`
/// at all — sanity check that the adapter isn't just downgrading severity
/// blindly on every file.
#[test]
fn clean_html_reports_no_parse_error() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("index.html"),
        "<html lang=\"en\"><body><p>Hello</p></body></html>\n",
    )
    .expect("write html");

    let v = parse_error_findings(dir.path(), "index.html");
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        !findings
            .iter()
            .any(|f| f["id"].as_str() == Some("Warning.ParseError")),
        "clean HTML must not report a parse error; findings={findings:?}"
    );
}
