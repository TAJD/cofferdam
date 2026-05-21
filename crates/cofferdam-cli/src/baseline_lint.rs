//! `cofferdam baseline lint` — report baseline entries also suppressed inline.
//!
//! A "dual-state" entry is one that is both in the baseline file AND covered
//! by an inline suppression directive.  Both silence the finding, but the
//! double-silencing is wasted signal and can confuse teams trying to
//! understand their technical-debt posture.
//!
//! Exit 1 if any dual-state entries are found, 0 otherwise (CI-wireable).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_engine::baseline::{self, Baseline, BaselineEntry, DEFAULT_PATH};
use cofferdam_engine::suppress::{self, SuppressionForm};
use serde::Serialize;

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DualStateEntry {
    pub file: String,
    pub line: u32,
    pub check_id: String,
    pub suppression_form: &'static str,
    pub suppression_text: String,
}

#[derive(Debug, Serialize)]
struct LintReport {
    dual_state: Vec<DualStateEntry>,
    summary: LintSummary,
}

#[derive(Debug, Serialize)]
struct LintSummary {
    count: usize,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub(crate) fn run(robot: bool, pretty: bool) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: could not determine working directory: {e}");
            return ExitCode::from(2);
        }
    };
    run_at(&cwd, robot, pretty)
}

/// Test-friendly variant: takes the directory to search from.
/// Avoids `std::env::set_current_dir` so parallel test threads don't race.
pub(crate) fn run_at(dir: &Path, robot: bool, pretty: bool) -> ExitCode {
    let baseline_path = match resolve_baseline_path_from(dir) {
        Some(p) => p,
        None => {
            if robot {
                let report = LintReport {
                    dual_state: vec![],
                    summary: LintSummary { count: 0 },
                };
                println!("{}", serialise(&report, pretty));
            } else {
                eprintln!("no baseline file found");
            }
            return ExitCode::SUCCESS;
        }
    };

    let baseline = match baseline::read(&baseline_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let project_root = baseline_path
        .parent() // .cofferdam/
        .and_then(|p| p.parent()) // project root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.to_path_buf());

    let entries = find_dual_state(&baseline, &project_root);
    let count = entries.len();

    if robot {
        let report = LintReport {
            summary: LintSummary { count },
            dual_state: entries,
        };
        println!("{}", serialise(&report, pretty));
    } else {
        for e in &entries {
            eprintln!(
                "~ {}:{} — {} is in the baseline AND suppressed inline",
                e.file, e.line, e.check_id
            );
            eprintln!("    ({})", e.suppression_text);
        }
        if count == 0 {
            eprintln!("0 dual-state entries found");
        } else {
            eprintln!("{count} dual-state entr(ies) found");
        }
    }

    if count > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ── Counting how many overlaps exist (for doctor) ─────────────────────────────

/// Return the count of dual-state (baseline + inline suppression) entries
/// from `dir`, without printing anything.  Returns 0 when no baseline exists
/// or when none overlap.  Returns `Err` on IO / parse failure.
pub(crate) fn count_dual_state_at(dir: &Path) -> Result<usize, String> {
    let baseline_path = match resolve_baseline_path_from(dir) {
        Some(p) => p,
        None => return Ok(0),
    };
    let baseline = baseline::read(&baseline_path).map_err(|e| e.to_string())?;
    let project_root = baseline_path
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.to_path_buf());
    Ok(find_dual_state(&baseline, &project_root).len())
}

// ── Core logic ────────────────────────────────────────────────────────────────

fn find_dual_state(baseline: &Baseline, project_root: &Path) -> Vec<DualStateEntry> {
    // Group baseline entries by file.
    let mut by_file: HashMap<&str, Vec<&BaselineEntry>> = HashMap::new();
    for entry in &baseline.findings {
        by_file.entry(entry.file.as_str()).or_default().push(entry);
    }

    let mut results: Vec<DualStateEntry> = Vec::new();

    for (file_rel, entries) in by_file {
        let abs_path: PathBuf =
            project_root.join(file_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let text = match std::fs::read_to_string(&abs_path) {
            Ok(t) => t,
            Err(_) => continue, // file deleted — not our concern here
        };

        let suppression_entries = suppress::parse_suppression_entries(&text);

        // Build a lookup: (check_id, line) → SuppressionEntry for O(1) intersection.
        // For file-wide suppressions (line == 0) we handle them separately.
        // For broad (empty check_id) suppressions we treat them as matching anything.
        for baseline_entry in entries {
            // Find a suppression that covers this baseline entry.
            // We don't have the line number in the baseline (only the signature),
            // so we scan all suppression entries for matching check_id and see if
            // any cover any line.  For a quick v1 we do a linear scan per entry;
            // the baseline is small enough that this is fine.
            if let Some(sup) = find_suppression(&suppression_entries, &baseline_entry.check_id) {
                let form_str = match sup.form {
                    SuppressionForm::NextLine => "next_line",
                    SuppressionForm::Range => "range",
                    SuppressionForm::File => "file",
                };
                results.push(DualStateEntry {
                    file: file_rel.to_string(),
                    line: sup.line,
                    check_id: baseline_entry.check_id.clone(),
                    suppression_form: form_str,
                    suppression_text: sup.directive_text.clone(),
                });
            }
        }
    }

    // Sort for stable output: file, then check_id, then line.
    results.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.check_id.cmp(&b.check_id))
            .then_with(|| a.line.cmp(&b.line))
    });
    results
}

fn find_suppression<'a>(
    entries: &'a [suppress::SuppressionEntry],
    check_id: &str,
) -> Option<&'a suppress::SuppressionEntry> {
    entries.iter().find(|e| {
        // Broad suppression (empty check_id) covers everything.
        // Specific suppression must match check_id exactly.
        e.check_id.is_empty() || e.check_id == check_id
    })
}

// ── Path resolution ───────────────────────────────────────────────────────────

fn resolve_baseline_path_from(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(DEFAULT_PATH);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn serialise<T: Serialize>(v: &T, pretty: bool) -> String {
    if pretty {
        serde_json::to_string_pretty(v)
    } else {
        serde_json::to_string(v)
    }
    .expect("serializes infallibly")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_engine::baseline::{Baseline, BaselineEntry};
    use tempfile::TempDir;

    fn write_baseline(dir: &Path, findings: Vec<BaselineEntry>) {
        let bl = Baseline::new(findings);
        let target = dir.join(".cofferdam/baseline.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        baseline::write(&target, &bl).unwrap();
    }

    fn make_entry(file: &str, check_id: &str) -> BaselineEntry {
        BaselineEntry {
            file: file.into(),
            check_id: check_id.into(),
            // Signature doesn't matter for lint — we match by (file, check_id, line).
            rule_signature: "sig".into(),
        }
    }

    fn is_success(code: ExitCode) -> bool {
        code == ExitCode::SUCCESS
    }

    #[test]
    fn lint_reports_dual_state() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Source file: has an inline suppression for Warning.NoConsoleLog.
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("demo.ts"),
            "// cofferdam-ignore: Warning.NoConsoleLog: legacy\nconsole.log(\"hi\");\n",
        )
        .unwrap();

        // Baseline: also contains Warning.NoConsoleLog for that file.
        write_baseline(
            root,
            vec![make_entry("src/demo.ts", "Warning.NoConsoleLog")],
        );

        let result = run_at(root, false, false);
        // Exit 1: dual-state entry found.
        assert!(!is_success(result), "expected exit 1 for dual-state entry");
    }

    #[test]
    fn lint_silent_without_overlap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        // Source file: no inline suppression.
        std::fs::write(src_dir.join("demo.ts"), "console.log(\"hi\");\n").unwrap();

        // Baseline: has Warning.NoConsoleLog but no inline directive.
        write_baseline(
            root,
            vec![make_entry("src/demo.ts", "Warning.NoConsoleLog")],
        );

        let result = run_at(root, false, false);
        assert!(is_success(result), "expected exit 0 when no overlap");
    }

    #[test]
    fn lint_passes_when_no_baseline() {
        let tmp = TempDir::new().unwrap();
        let result = run_at(tmp.path(), false, false);
        assert!(is_success(result), "no baseline → exit 0");
    }

    #[test]
    fn lint_count_dual_state_returns_one_for_overlap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("demo.ts"),
            "// cofferdam-ignore: Warning.NoConsoleLog: legacy\nconsole.log(\"hi\");\n",
        )
        .unwrap();
        write_baseline(
            root,
            vec![make_entry("src/demo.ts", "Warning.NoConsoleLog")],
        );

        let count = count_dual_state_at(root).unwrap();
        assert_eq!(count, 1, "one dual-state entry");
    }
}
