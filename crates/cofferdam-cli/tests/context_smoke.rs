//! Integration tests for `cofferdam context`.
//!
//! `Context.Findings` (CD-159) is the first registered provider; these
//! tests assert the invocation contract (exit codes, changeset
//! resolution modes, JSON shape) using fixtures deliberately chosen not
//! to trip any builtin check (e.g. non-exported bindings, so
//! `Design.OrphanExport` doesn't fire), so the assertions stay about the
//! contract rather than any particular provider's output.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Cofferdam Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

const GIT_ENV_VARS_TO_CLEAR: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
];

fn run_git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(dir: &Path, message: &str) {
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "--quiet", "--no-verify", "-m", message]);
}

fn cofferdam_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cofferdam")
}

fn cofferdam_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(cofferdam_bin());
    cmd.current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn context_on_clean_tree_reports_no_changes_and_exits_zero() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No relevant context found for 0 changed file(s)."),
        "got: {stdout}"
    );
}

#[test]
fn context_with_working_tree_edit_exits_zero_with_digest_or_empty_message() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    let file = dir.join("a.ts");
    std::fs::write(&file, "export const x = 1;\n").expect("write");
    commit_all(dir, "init");
    std::fs::write(&file, "export const x = 2;\n").expect("edit");

    let out = cofferdam_cmd(dir)
        .args(["context"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("changed file(s)"), "got: {stdout}");
}

#[test]
fn context_json_shape_is_stable() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    // Non-exported binding: nothing for a builtin check (e.g.
    // `Design.OrphanExport`) to flag, so the digest stays empty and this
    // test can assert on JSON shape alone, independent of provider output.
    let file = dir.join("a.ts");
    std::fs::write(&file, "const x = 1;\n").expect("write");
    commit_all(dir, "init");
    std::fs::write(&file, "const x = 2;\n").expect("edit");

    let out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["changed_files"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["items"].as_array().unwrap().len(), 0);
    assert_eq!(parsed["omitted"], 0);
    assert_eq!(parsed["budget"], 2000);
}

#[test]
fn context_explicit_files_work_without_git() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");

    let out = cofferdam_cmd(dir)
        .args(["context", "a.ts"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 changed file(s)"), "got: {stdout}");
}

/// CD-202: explicit-path mode must resolve the same absolute paths the
/// engine's discovery pass produces, and must discover from the repo
/// root (not cwd) so the cross-file graph the `Context.BlastRadius`
/// provider needs is actually built. Regression test: before the fix,
/// `std::fs::canonicalize` on Windows produced `\\?\`-prefixed paths
/// that never equalled the (unprefixed) discovered paths, so explicit-
/// path mode silently returned zero items while git-diff mode — same
/// file, same repo — returned the real digest.
#[test]
fn context_explicit_path_matches_git_diff_mode_on_blast_radius_fixture() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("blast_radius");
    for entry in std::fs::read_dir(&fixture_dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        std::fs::copy(entry.path(), dir.join(&name)).expect("copy fixture file");
    }
    commit_all(dir, "init");

    // Synthetic signature change to `doThing`, matching the blast-radius
    // fixture's documented scenario (see examples/blast_radius/lib.ts).
    std::fs::write(
        dir.join("lib.ts"),
        "export function doThing(x: number, y: number): string {\n  return String(x + y);\n}\n",
    )
    .expect("edit lib.ts");

    let git_diff_out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam (git-diff mode)");
    assert!(
        git_diff_out.status.success(),
        "git-diff mode: expected exit 0; stderr={}",
        String::from_utf8_lossy(&git_diff_out.stderr)
    );

    let explicit_out = cofferdam_cmd(dir)
        .args(["context", "lib.ts", "--format", "json"])
        .output()
        .expect("invoke cofferdam (explicit-path mode)");
    assert!(
        explicit_out.status.success(),
        "explicit-path mode: expected exit 0; stderr={}",
        String::from_utf8_lossy(&explicit_out.stderr)
    );

    let git_diff_json: serde_json::Value =
        serde_json::from_slice(&git_diff_out.stdout).expect("valid JSON (git-diff mode)");
    let explicit_json: serde_json::Value =
        serde_json::from_slice(&explicit_out.stdout).expect("valid JSON (explicit-path mode)");

    let git_diff_items = git_diff_json["items"].as_array().expect("items array");
    assert!(
        !git_diff_items.is_empty(),
        "expected non-empty items in git-diff mode; got {git_diff_json}"
    );
    assert_eq!(
        git_diff_json["items"], explicit_json["items"],
        "explicit-path mode must yield the same items as git-diff mode for the same change"
    );
}

/// CD-164 criterion 4: `cofferdam check`'s output must be byte-for-byte
/// unaffected by the existence of `Category::Context` providers —
/// `Cmd::Check` only ever constructs the engine from `all_builtins()`
/// (verified statically: `all_context_providers()` is referenced nowhere
/// in `cofferdam-cli` outside `context_cmd.rs`). This test pins that
/// behavior at the black-box level: two `cofferdam check` runs against a
/// fixture that trips a real builtin finding must produce identical
/// output, and that output must never mention a `Context.*` check id.
#[test]
fn check_output_is_byte_for_byte_unaffected_by_context_providers() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    // `==` trips `Warning.TripleEquals`, a real builtin finding.
    std::fs::write(dir.join("a.ts"), "export function f(x: number) {\n  if (x == 1) {\n    return true;\n  }\n  return false;\n}\n").expect("write");
    commit_all(dir, "init");

    let run = || {
        let out = cofferdam_cmd(dir)
            .args(["check", "--format", "json"])
            .output()
            .expect("invoke cofferdam check");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "cofferdam check output must be deterministic across repeated runs"
    );
    assert!(
        first.contains("Warning.TripleEquals"),
        "expected fixture to trip a real builtin finding; got: {first}"
    );
    assert!(
        !first.contains("\"Context."),
        "cofferdam check output must never contain a Context.* check id; got: {first}"
    );
}

/// CD-210: `Context.Precedent`'s score must rank strictly below every
/// other provider's, always — the shared `cofferdam_core::relevance`
/// scale is supposed to guarantee this by construction (see
/// `relevance::FLOOR`'s doc comment), not by coincidence. This fixture
/// deliberately triggers both `Context.BlastRadius` (a direct importer
/// of the changed file) and `Context.Precedent` (two shape-sharing
/// siblings in the same directory as the changed file) in one
/// changeset, so the two providers' items land in the same digest and
/// the invariant is checked against real, end-to-end output rather
/// than unit-level constants alone.
#[test]
fn precedent_score_ranks_below_every_other_provider_in_the_same_digest() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    let handlers = dir.join("handlers");
    std::fs::create_dir(&handlers).expect("mkdir handlers");

    std::fs::write(
        handlers.join("create_user.ts"),
        "export interface CreateUserRequest { name: string; email: string; role: string; orgId: string; }\n\
         export async function createUser(req: CreateUserRequest): Promise<string> { return req.name; }\n",
    )
    .expect("write create_user.ts");
    std::fs::write(
        handlers.join("update_user.ts"),
        "export interface UpdateUserRequest { id: string; name: string; email: string; role: string; orgId: string; }\n\
         export async function updateUser(req: UpdateUserRequest): Promise<string> { return req.id; }\n",
    )
    .expect("write update_user.ts");
    std::fs::write(
        handlers.join("list_users.ts"),
        "export interface ListUsersRequest { orgId: string; role: string; name: string; email: string; }\n\
         export async function listUsers(req: ListUsersRequest): Promise<string[]> { return []; }\n",
    )
    .expect("write list_users.ts");
    std::fs::write(
        handlers.join("consumer.ts"),
        "import { createUser } from './create_user';\nexport async function handle() { await createUser({ name: 'a', email: 'b', role: 'c', orgId: 'd' }); }\n",
    )
    .expect("write consumer.ts");
    commit_all(dir, "init");

    // Edit create_user.ts without changing its export shape, so the
    // Precedent cluster (update_user.ts + list_users.ts) still applies
    // to it, while also making it a changed file that consumer.ts
    // directly imports (BlastRadius).
    std::fs::write(
        handlers.join("create_user.ts"),
        "export interface CreateUserRequest { name: string; email: string; role: string; orgId: string; }\n\
         export async function createUser(req: CreateUserRequest): Promise<string> {\n  return req.name;\n}\n",
    )
    .expect("edit create_user.ts");

    let out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let items = json["items"].as_array().expect("items array");

    let precedent_scores: Vec<i64> = items
        .iter()
        .filter(|i| i["check_id"] == "Context.Precedent")
        .map(|i| i["score"].as_i64().expect("score is an integer"))
        .collect();
    let other_scores: Vec<i64> = items
        .iter()
        .filter(|i| i["check_id"] != "Context.Precedent")
        .map(|i| i["score"].as_i64().expect("score is an integer"))
        .collect();

    assert!(
        !precedent_scores.is_empty(),
        "expected the fixture to trigger a Context.Precedent item; got {items:?}"
    );
    assert!(
        !other_scores.is_empty(),
        "expected the fixture to trigger at least one non-Precedent item (e.g. BlastRadius); got {items:?}"
    );

    let precedent_max = precedent_scores.iter().max().copied().unwrap();
    let others_min = other_scores.iter().min().copied().unwrap();
    assert!(
        precedent_max < others_min,
        "Context.Precedent's highest score ({precedent_max}) must rank strictly below every \
         other provider's lowest score ({others_min}) in the same digest; got items {items:?}"
    );
    assert_eq!(
        precedent_max,
        i64::from(cofferdam_core::relevance::FLOOR),
        "Context.Precedent must emit exactly relevance::FLOOR"
    );
}

/// CD-212: a `[[context_suppress]]` rule targeting `Context.Precedent`
/// over a directory must drop that provider's item from the digest,
/// even though the underlying sibling-convention inference still
/// fires (the provider itself is untouched; suppression is a
/// post-filter).
#[test]
fn context_suppress_rule_drops_matching_provider_items_from_the_digest() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    let handlers = dir.join("handlers");
    std::fs::create_dir(&handlers).expect("mkdir handlers");
    std::fs::write(
        handlers.join("create_user.ts"),
        "export interface CreateUserRequest { name: string; email: string; role: string; orgId: string; }\n\
         export async function createUser(req: CreateUserRequest): Promise<string> { return req.name; }\n",
    )
    .expect("write create_user.ts");
    std::fs::write(
        handlers.join("update_user.ts"),
        "export interface UpdateUserRequest { id: string; name: string; email: string; role: string; orgId: string; }\n\
         export async function updateUser(req: UpdateUserRequest): Promise<string> { return req.id; }\n",
    )
    .expect("write update_user.ts");
    std::fs::write(
        handlers.join("list_users.ts"),
        "export interface ListUsersRequest { orgId: string; role: string; name: string; email: string; }\n\
         export async function listUsers(req: ListUsersRequest): Promise<string[]> { return []; }\n",
    )
    .expect("write list_users.ts");
    std::fs::write(
        dir.join("cofferdam.toml"),
        "[[context_suppress]]\ncheck_id = \"Context.Precedent\"\npaths = [\"handlers/**\"]\nreason = \"noise for this fixture\"\n",
    )
    .expect("write cofferdam.toml");
    commit_all(dir, "init");

    std::fs::write(
        handlers.join("delete_user.ts"),
        "export async function deleteUser(id: string): Promise<void> {}\n",
    )
    .expect("write delete_user.ts");

    let out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let items = json["items"].as_array().expect("items array");
    assert!(
        !items.iter().any(|i| i["check_id"] == "Context.Precedent"),
        "context_suppress rule should have dropped the Context.Precedent item; got {items:?}"
    );
}

/// CD-212: `--lint-context-suppress` flags a rule whose `paths` globs
/// match zero files in the current repo as likely-stale, and exits 0
/// when every rule matches at least one file.
#[test]
fn lint_context_suppress_flags_a_rule_matching_zero_files() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::write(dir.join("real.ts"), "export const x = 1;\n").expect("write real.ts");
    std::fs::write(
        dir.join("cofferdam.toml"),
        "[[context_suppress]]\ncheck_id = \"Context.Precedent\"\npaths = [\"src/nonexistent/**\"]\n",
    )
    .expect("write cofferdam.toml");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context", "--lint-context-suppress"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        !out.status.success(),
        "expected nonzero exit for a stale rule"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("matches 0 files"),
        "expected a stale-rule diagnostic; got stderr={stderr}"
    );
}

#[test]
fn lint_context_suppress_passes_when_every_rule_matches_a_file() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::create_dir(dir.join("src")).expect("mkdir src");
    std::fs::write(dir.join("src/real.ts"), "export const x = 1;\n").expect("write real.ts");
    std::fs::write(
        dir.join("cofferdam.toml"),
        "[[context_suppress]]\ncheck_id = \"Context.Precedent\"\npaths = [\"src/**\"]\n",
    )
    .expect("write cofferdam.toml");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context", "--lint-context-suppress"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 rule(s) OK"), "got stdout={stdout}");
}

/// CD-164 criterion 2: `cofferdam context` digests must be deterministic
/// (golden-snapshot precondition) — two runs over the same changeset
/// produce byte-identical JSON, including item ordering.
#[test]
fn context_digest_is_deterministic_across_repeated_runs() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("blast_radius");
    for entry in std::fs::read_dir(&fixture_dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        std::fs::copy(entry.path(), dir.join(&name)).expect("copy fixture file");
    }
    commit_all(dir, "init");
    std::fs::write(
        dir.join("lib.ts"),
        "export function doThing(x: number, y: number): string {\n  return String(x + y);\n}\n",
    )
    .expect("edit lib.ts");

    let run = || {
        let out = cofferdam_cmd(dir)
            .args(["context", "--format", "json"])
            .output()
            .expect("invoke cofferdam context");
        assert!(
            out.status.success(),
            "expected exit 0; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let first = run();
    let second = run();
    assert_eq!(
        first, second,
        "cofferdam context digest must be byte-for-byte deterministic across repeated runs"
    );
    let parsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
    assert!(
        !parsed["items"].as_array().unwrap().is_empty(),
        "expected a non-empty digest for this fixture"
    );
}

#[test]
fn context_bad_base_ref_is_a_usage_error() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context", "--base", "no-such-ref"])
        .output()
        .expect("invoke cofferdam");

    assert!(!out.status.success(), "expected nonzero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.is_empty(), "expected a diagnostic on stderr");
}
