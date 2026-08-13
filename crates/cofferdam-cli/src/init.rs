//! `cofferdam init` — first-run scaffolding.
//!
//! Bridges the install-to-build-step gap: writes a starter
//! `cofferdam.toml`, optionally captures a baseline of current
//! findings, and merges the right entries into `.gitignore`. After
//! `init`, the user has a working `cofferdam check` they can drop into
//! CI without immediately turning red.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_checks::all_builtins;
use cofferdam_engine::baseline::{self, Baseline};
use cofferdam_engine::{discover, DiscoveryOptions, Engine};

/// Starter `cofferdam.toml`. Deliberately minimal: every check stanza
/// is commented out so users can see the surface area without the file
/// looking like noise. Defaults all match what the binary already
/// applies — uncommenting a stanza and tweaking a value is the only
/// supported edit path.
pub const STARTER_TOML: &str = "\
# cofferdam.toml — code-quality analyzer config.
# Each [checks.\"X.Y\"] block overrides the built-in defaults for one
# check. Uncomment and edit the values you actually want to change;
# missing checks use their built-in defaults.
#
# Severity gating (medium and above fails CI by default — change with
# `cofferdam check --fail-on=<level>`):
#   info | low | medium | high | critical

# [checks.\"Readability.MaxLineLength\"]
# limit = 120
# severity = \"low\"

# [checks.\"Readability.MaxFunctionLength\"]
# limit = 50
# severity = \"low\"

# [checks.\"Design.MaxParameters\"]
# limit = 5
# severity = \"medium\"

# [checks.\"Design.DuplicateExportName\"]
# severity = \"medium\"

# [checks.\"Refactor.NearDuplicateBlock\"]
# severity = \"medium\"

# [checks.\"Warning.TripleEquals\"]
# severity = \"high\"

# [checks.\"Consistency.QuoteStyle\"]
# severity = \"low\"
";

/// What `init` should do about a baseline. `Auto` triggers an
/// interactive `[Y/n]` prompt at TTY; `Yes`/`No` skip it (set by
/// `--baseline` / `--no-baseline`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BaselineChoice {
    Auto,
    Yes,
    No,
}

pub struct InitArgs {
    pub path: PathBuf,
    pub force: bool,
    pub baseline_choice: BaselineChoice,
    pub robot: bool,
}

/// Result of an `init` run, used by both the human and robot output
/// paths so they describe the same outcome.
struct InitOutcome {
    config_path: PathBuf,
    config_written: bool,
    baseline_path: Option<PathBuf>,
    baseline_finding_count: Option<usize>,
    gitignore_path: PathBuf,
    gitignore_changed: bool,
}

pub fn run(args: InitArgs) -> ExitCode {
    let InitArgs {
        path,
        force,
        baseline_choice,
        robot,
    } = args;

    if !path.is_dir() {
        eprintln!("error: {} is not a directory", path.display());
        return ExitCode::from(2);
    }

    let config_path = path.join("cofferdam.toml");
    let baseline_path = path.join(baseline::DEFAULT_PATH);
    let gitignore_path = path.join(".gitignore");

    if config_path.exists() && !force {
        eprintln!(
            "error: {} already exists. Pass --force to overwrite.",
            config_path.display()
        );
        return ExitCode::from(1);
    }

    if let Err(e) = std::fs::write(&config_path, STARTER_TOML) {
        eprintln!("error: writing {}: {e}", config_path.display());
        return ExitCode::from(2);
    }

    let want_baseline = match baseline_choice {
        BaselineChoice::Yes => true,
        BaselineChoice::No => false,
        BaselineChoice::Auto => {
            if robot {
                // No prompts in robot mode — default to yes since that's
                // the friendlier first-run choice.
                true
            } else {
                prompt_baseline(&path)
            }
        }
    };

    let baseline_finding_count = if want_baseline {
        match write_baseline_for_init(&path, &baseline_path) {
            Ok(n) => Some(n),
            Err(e) => {
                eprintln!("error: writing baseline: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };

    let gitignore_changed = match merge_gitignore(&gitignore_path) {
        Ok(changed) => changed,
        Err(e) => {
            eprintln!("error: updating {}: {e}", gitignore_path.display());
            return ExitCode::from(2);
        }
    };

    let outcome = InitOutcome {
        config_path,
        config_written: true,
        baseline_path: if want_baseline {
            Some(baseline_path)
        } else {
            None
        },
        baseline_finding_count,
        gitignore_path,
        gitignore_changed,
    };

    if robot {
        print_robot(&outcome);
    } else {
        print_human(&outcome);
    }

    ExitCode::SUCCESS
}

fn prompt_baseline(path: &Path) -> bool {
    // Count what would land in the baseline so the prompt is concrete.
    // If discovery or the engine fails (e.g. no TS files), default to
    // "yes" — writing an empty baseline isn't harmful and the user
    // probably wants the file in place for when they add code.
    let opts = DiscoveryOptions::default();
    let count = discover(&[path], &opts)
        .ok()
        .and_then(|files| {
            if files.is_empty() {
                return None;
            }
            let engine = Engine::new(all_builtins());
            engine.analyze(&files).ok().map(|v| v.len())
        })
        .unwrap_or(0);

    eprint!(
        "Create a baseline of {} current finding{} so they don't fail CI? [Y/n] ",
        count,
        if count == 1 { "" } else { "s" }
    );
    let _ = io::stderr().flush();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        // No stdin (likely non-interactive after all) — default yes.
        return true;
    }
    let trimmed = answer.trim().to_ascii_lowercase();
    trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
}

fn write_baseline_for_init(root: &Path, target: &Path) -> Result<usize, String> {
    let opts = DiscoveryOptions::default();
    let files = discover(&[root], &opts).map_err(|e| e.to_string())?;
    if files.is_empty() {
        // Empty baseline still gets written so the file exists and
        // CI is gated on "no new findings" from the start.
        let baseline = Baseline::new(Vec::new());
        baseline::write(target, &baseline).map_err(|e| e.to_string())?;
        return Ok(0);
    }
    let engine = Engine::new(all_builtins());
    let signed = engine
        .analyze_with_signatures(&files)
        .map_err(|e| e.to_string())?;
    let entries: Vec<_> = signed
        .iter()
        .map(|(issue, sig)| baseline::entry_for(issue, sig.clone(), Some(root)))
        .collect();
    let baseline = Baseline::new(entries);
    let count = baseline.findings.len();
    baseline::write(target, &baseline).map_err(|e| e.to_string())?;
    Ok(count)
}

/// Append the cofferdam-specific entries to `.gitignore`, creating it
/// if missing. Returns `true` when the file changed.
///
/// We add `.cofferdam/*` (ignore the directory's contents file-by-file)
/// and negate `!.cofferdam/baseline.json` — the baseline file is committed
/// (it's the project's record of accepted findings); other entries
/// underneath are gitignored.
///
/// Subtle: per gitignore semantics, excluding a directory with `dir/`
/// means git will not even look inside it — so a later
/// `!dir/baseline.json` cannot un-ignore the file. The `dir/*` form
/// matches files individually, which lets the negation reach.
/// 0.2.x scaffolded the broken `.cofferdam/` form (cd-6we / gh #4); if
/// we see it alongside the negation, we migrate it to `.cofferdam/*`.
pub fn merge_gitignore(path: &Path) -> io::Result<bool> {
    const DIR_GLOB: &str = ".cofferdam/*";
    const DIR_BROKEN: &str = ".cofferdam/";
    const NEGATE_BASELINE: &str = "!.cofferdam/baseline.json";
    const ENTRIES: &[&str] = &[DIR_GLOB, NEGATE_BASELINE];

    let existing = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let trimmed_lines: Vec<&str> = existing.lines().map(str::trim).collect();
    let has_broken = trimmed_lines.contains(&DIR_BROKEN);
    let has_glob = trimmed_lines.contains(&DIR_GLOB);
    let has_negate = trimmed_lines.contains(&NEGATE_BASELINE);

    // Detect the 0.2.x broken state and migrate in place: rewrite
    // `.cofferdam/` to `.cofferdam/*` so the baseline-negate works.
    let needs_migration = has_broken && !has_glob;

    let already: std::collections::HashSet<&str> = trimmed_lines.iter().copied().collect();
    let to_add: Vec<&str> = ENTRIES
        .iter()
        .copied()
        .filter(|e| {
            // After migration, the glob is effectively present; don't
            // also append it.
            if *e == DIR_GLOB && (has_glob || needs_migration) {
                return false;
            }
            if *e == NEGATE_BASELINE && has_negate {
                return false;
            }
            !already.contains(e)
        })
        .collect();

    if !needs_migration && to_add.is_empty() {
        return Ok(false);
    }

    let mut new = if needs_migration {
        // Rewrite the broken bare-directory line to the glob form,
        // preserving the rest of the file unchanged.
        let mut out = String::with_capacity(existing.len() + 2);
        let trailing_newline = existing.ends_with('\n');
        let mut iter = existing.lines().peekable();
        while let Some(line) = iter.next() {
            if line.trim() == DIR_BROKEN {
                out.push_str(DIR_GLOB);
            } else {
                out.push_str(line);
            }
            if iter.peek().is_some() || trailing_newline {
                out.push('\n');
            }
        }
        out
    } else {
        existing.clone()
    };

    if !to_add.is_empty() {
        if !new.is_empty() && !new.ends_with('\n') {
            new.push('\n');
        }
        if !new.is_empty() {
            new.push_str("\n# cofferdam\n");
        } else {
            new.push_str("# cofferdam\n");
        }
        for line in &to_add {
            new.push_str(line);
            new.push('\n');
        }
    }
    std::fs::write(path, new)?;
    Ok(true)
}

fn print_human(outcome: &InitOutcome) {
    if outcome.config_written {
        println!("✓ wrote {}", outcome.config_path.display());
    }
    if outcome.gitignore_changed {
        println!("✓ updated {}", outcome.gitignore_path.display());
    }
    if let Some(path) = &outcome.baseline_path {
        match outcome.baseline_finding_count {
            Some(n) => println!(
                "✓ wrote {} ({} finding{} baselined)",
                path.display(),
                n,
                if n == 1 { "" } else { "s" }
            ),
            None => println!("✓ wrote {}", path.display()),
        }
    }
    println!();
    println!("Next steps:");
    println!("  cofferdam check src/");
    println!();
    println!("Add to package.json:");
    println!("  \"scripts\": {{ \"lint\": \"cofferdam check src/\" }}");
}

fn print_robot(outcome: &InitOutcome) {
    let mut bag = serde_json::Map::new();
    bag.insert(
        "config_path".into(),
        serde_json::Value::String(outcome.config_path.display().to_string()),
    );
    bag.insert(
        "config_written".into(),
        serde_json::Value::Bool(outcome.config_written),
    );
    bag.insert(
        "gitignore_path".into(),
        serde_json::Value::String(outcome.gitignore_path.display().to_string()),
    );
    bag.insert(
        "gitignore_changed".into(),
        serde_json::Value::Bool(outcome.gitignore_changed),
    );
    if let Some(path) = &outcome.baseline_path {
        bag.insert(
            "baseline_path".into(),
            serde_json::Value::String(path.display().to_string()),
        );
    }
    if let Some(n) = outcome.baseline_finding_count {
        bag.insert(
            "baseline_finding_count".into(),
            serde_json::Value::Number(n.into()),
        );
    }
    let value = serde_json::Value::Object(bag);
    println!("{}", value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn merge_gitignore_creates_when_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gitignore");
        let changed = merge_gitignore(&path).unwrap();
        assert!(changed);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(".cofferdam/*"));
        assert!(content.contains("!.cofferdam/baseline.json"));
        // Must be the glob form, not the bare directory form (see
        // cd-6we / gh #4 — bare-directory exclusion blocks the negate).
        assert!(!content.lines().any(|l| l.trim() == ".cofferdam/"));
    }

    #[test]
    fn merge_gitignore_appends_to_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, "node_modules/\n.env\n").unwrap();
        let changed = merge_gitignore(&path).unwrap();
        assert!(changed);
        let content = std::fs::read_to_string(&path).unwrap();
        // Existing entries preserved.
        assert!(content.starts_with("node_modules/\n.env\n"));
        assert!(content.contains(".cofferdam/*"));
        assert!(content.contains("!.cofferdam/baseline.json"));
    }

    #[test]
    fn merge_gitignore_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(&path, ".cofferdam/*\n!.cofferdam/baseline.json\n").unwrap();
        let changed = merge_gitignore(&path).unwrap();
        assert!(!changed);
    }

    #[test]
    fn merge_gitignore_partial_existing_only_adds_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gitignore");
        // Only the glob is there; the negation is missing.
        std::fs::write(&path, ".cofferdam/*\n").unwrap();
        let changed = merge_gitignore(&path).unwrap();
        assert!(changed);
        let content = std::fs::read_to_string(&path).unwrap();
        // Original line preserved exactly once; only the negate appended.
        assert_eq!(content.matches(".cofferdam/*").count(), 1);
        assert!(content.contains("!.cofferdam/baseline.json"));
    }

    #[test]
    fn merge_gitignore_migrates_broken_bare_directory_form() {
        // 0.2.x scaffolded `.cofferdam/` + `!.cofferdam/baseline.json`,
        // which is the broken state per cd-6we. Re-running init must
        // rewrite the bare-directory line to the glob form so the
        // negate actually un-ignores baseline.json.
        let dir = tempdir().unwrap();
        let path = dir.path().join(".gitignore");
        std::fs::write(
            &path,
            "node_modules/\n\n# cofferdam\n.cofferdam/\n!.cofferdam/baseline.json\n",
        )
        .unwrap();
        let changed = merge_gitignore(&path).unwrap();
        assert!(changed, "must migrate the broken state");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(".cofferdam/*"), "wrote: {content:?}");
        assert!(content.contains("!.cofferdam/baseline.json"));
        assert!(
            !content.lines().any(|l| l.trim() == ".cofferdam/"),
            "bare-directory line must be removed; got: {content:?}"
        );
        // Surrounding content preserved.
        assert!(content.starts_with("node_modules/"));
    }

    /// End-to-end: in a real git repo, after `init --baseline`'s gitignore
    /// merge, `git check-ignore` must report `.cofferdam/baseline.json`
    /// as NOT ignored. Skip silently if git isn't on PATH (rare in CI,
    /// but we don't want to flake locally on minimal images).
    #[test]
    fn baseline_json_is_not_gitignored_after_merge() {
        let Ok(git_check) = std::process::Command::new("git").arg("--version").output() else {
            eprintln!("git not on PATH — skipping check-ignore smoke");
            return;
        };
        if !git_check.status.success() {
            eprintln!("git --version failed — skipping check-ignore smoke");
            return;
        }

        let dir = tempdir().unwrap();
        let repo = dir.path();

        // Fresh repo, no commits needed for check-ignore.
        let init = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(init.success(), "git init failed");

        // Run our merge.
        let gitignore = repo.join(".gitignore");
        merge_gitignore(&gitignore).unwrap();

        // Path must exist for check-ignore to evaluate it on some git
        // versions; create the directory and a stub baseline file.
        std::fs::create_dir_all(repo.join(".cofferdam")).unwrap();
        std::fs::write(repo.join(".cofferdam/baseline.json"), "{}\n").unwrap();

        // `git check-ignore` (no `-v`) exit codes:
        //   0 → path would be ignored
        //   1 → path would not be ignored (no rule, or negation wins)
        // With `-v`, git treats a matching negation as a "match" and
        // exits 0 — wrong signal for our test. Use the plain form.
        let out = std::process::Command::new("git")
            .args(["check-ignore", ".cofferdam/baseline.json"])
            .current_dir(repo)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(1),
            "baseline.json should NOT be gitignored — \
             exit={:?} stdout={stdout:?} stderr={stderr:?} \
             gitignore={:?}",
            out.status.code(),
            std::fs::read_to_string(&gitignore).unwrap(),
        );

        // Sanity: a peer file under .cofferdam/ SHOULD still be ignored.
        std::fs::write(repo.join(".cofferdam/cache.bin"), b"x").unwrap();
        let peer = std::process::Command::new("git")
            .args(["check-ignore", ".cofferdam/cache.bin"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert_eq!(
            peer.status.code(),
            Some(0),
            "peer files under .cofferdam/ should still be ignored",
        );
    }

    #[test]
    fn starter_toml_is_well_formed() {
        // Round-trip parse to confirm the comment-heavy file actually
        // produces valid TOML (i.e. all stanzas really are comments).
        let parsed: toml::Value = toml::from_str(STARTER_TOML).expect("parses");
        // No checks defined when everything is commented out — sanity
        // check that we haven't accidentally left a stanza live.
        assert!(parsed.as_table().map(|t| t.is_empty()).unwrap_or(true));
    }
}
