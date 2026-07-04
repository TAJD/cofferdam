//! Spec-contract fixture tests: the "good" package trips nothing, the
//! "bad" package trips every one of the 11 built-in checks.

use std::collections::BTreeSet;
use std::path::Path;

use cofferdam_typst::{all_typst_checks, load};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_all(root: &Path) -> Vec<cofferdam_core::Issue> {
    let pkg = load(root).expect("fixture package should load");
    all_typst_checks()
        .iter()
        .flat_map(|c| c.check(&pkg))
        .collect()
}

#[test]
fn good_fixture_is_clean() {
    let issues = run_all(&fixture("good"));
    assert!(
        issues.is_empty(),
        "expected zero findings on the good fixture, got: {:#?}",
        issues.iter().map(|i| &i.check_id).collect::<Vec<_>>()
    );
}

#[test]
fn bad_fixture_trips_every_check() {
    let bad_root = fixture("bad/preview/some-pkg/9.9.9");
    let issues = run_all(&bad_root);

    let expected: BTreeSet<&str> = all_typst_checks().iter().map(|c| c.meta().id).collect();
    let found: BTreeSet<&str> = issues.iter().map(|i| i.check_id.as_str()).collect();

    let missing: Vec<&&str> = expected.difference(&found).collect();
    assert!(
        missing.is_empty(),
        "bad fixture failed to trip check id(s): {missing:?}\nfindings were: {:#?}",
        issues.iter().map(|i| &i.check_id).collect::<Vec<_>>()
    );
}

#[test]
fn every_check_id_is_namespaced_typst_dot() {
    for check in all_typst_checks() {
        assert!(
            check.meta().id.starts_with("Typst."),
            "check id {} must be namespaced Typst.<Name>",
            check.meta().id
        );
    }
}
