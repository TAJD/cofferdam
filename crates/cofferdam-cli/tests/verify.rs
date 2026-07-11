//! `cofferdam verify --dist <dir>` (CD-85) — opt-in check mode for built
//! HTML output. Asserts findings are produced and labeled with their
//! build-output provenance, and that a plain `cofferdam check` run is
//! completely blind to the same dist tree.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn verify_dist_reports_labeled_findings() {
    let out = Command::new(cofferdam_bin())
        .args([
            "verify",
            "--dist",
            "examples/dist-fixture",
            "--format",
            "json",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cofferdam verify");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Html.MissingLangAttribute"),
        "expected a Html.MissingLangAttribute finding; stdout={stdout}"
    );
    assert!(
        stdout.contains(r#""origin":"build_output""#),
        "expected origin: build_output in JSON output; stdout={stdout}"
    );
    assert!(
        stdout.contains(r#""dist":"#),
        "expected a dist field in JSON output; stdout={stdout}"
    );
}

// `Check::output_mode() == true` only ADDS eligibility for `verify
// --dist` — see the doc comment on `Check::output_mode` — it never
// removes a check from `cofferdam check`'s normal dispatch. So
// `Html.MissingLangAttribute` fires under `cofferdam check
// examples/dist-fixture` exactly as it did before CD-85 (CD-85 never
// touches `run_check`/discovery for the default command at all); the
// isolation CD-85 actually guarantees is that verify --dist's own
// discovery+origin-tagged pipeline is a wholly separate code path, and
// that a normal `cofferdam check` invocation with no explicit dist
// argument doesn't discover an (ordinarily gitignored) dist directory
// on its own — not that pointing `check` explicitly at a directory
// full of `.html` files suppresses findings there.
#[test]
fn plain_check_still_applies_normal_checks_to_dist_fixture_html() {
    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "examples/dist-fixture",
            "--format",
            "json",
            "--no-baseline",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cofferdam check");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Html.MissingLangAttribute"),
        "output-mode-eligible checks must still run normally under plain `cofferdam check`; stdout={stdout}"
    );
}
