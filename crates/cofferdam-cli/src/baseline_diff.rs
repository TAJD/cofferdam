//! `cofferdam baseline diff` — compute delta between two baselines (or
//! current findings vs. the on-disk baseline).
//!
//! Renders the same delta block as `baseline write` does when a prior
//! baseline exists.  Useful for triage outside the rewrite path.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cofferdam_engine::baseline::{self, Baseline, Delta};
use serde::Serialize;

const BY_CHECK_LIMIT: usize = 10;

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub(crate) struct DeltaJson {
    pub added: usize,
    pub removed: usize,
    pub by_check: std::collections::BTreeMap<String, i64>,
}

#[derive(Debug, Serialize)]
struct DiffReport {
    delta: DeltaJson,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub(crate) struct DiffArgs {
    /// First baseline path. If `None`, defaults to `.cofferdam/baseline.json`.
    pub a: Option<PathBuf>,
    /// Second baseline path. If `Some`, compare `a` vs `b` directly.
    /// If `None`, the caller must supply `current` below.
    pub b: Option<PathBuf>,
    pub robot: bool,
    pub pretty: bool,
}

pub(crate) fn run(args: DiffArgs) -> ExitCode {
    match (&args.a, &args.b) {
        (Some(a), Some(b)) => {
            // Two explicit paths: compare a vs b.
            let prior = match load(a) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };
            let current = match load(b) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::from(2);
                }
            };
            let delta = baseline::compute_delta(&prior, &current);
            render(delta, args.robot, args.pretty);
            ExitCode::SUCCESS
        }
        // Any other combination (no args, or only b, or only a) requires an
        // engine run which is out of scope for v1.  Emit a helpful error.
        _ => {
            eprintln!(
                "error: `baseline diff` requires two baseline paths:\n  cofferdam baseline diff <a> <b>"
            );
            ExitCode::from(2)
        }
    }
}

// ── Rendering ────────────────────────────────────────────────────────────────

/// Render a `Delta` to stdout in text or JSON form.
pub(crate) fn render(delta: Delta, robot: bool, pretty: bool) {
    if robot {
        let report = DiffReport {
            delta: DeltaJson {
                added: delta.added,
                removed: delta.removed,
                by_check: delta.by_check.clone(),
            },
        };
        let s = if pretty {
            serde_json::to_string_pretty(&report)
        } else {
            serde_json::to_string(&report)
        }
        .expect("serializes infallibly");
        println!("{s}");
    } else {
        render_text(&delta);
    }
}

/// Render the delta summary block (human-readable, to stderr).
pub(crate) fn render_delta_text(delta: &Delta) {
    let net: i64 = delta.added as i64 - delta.removed as i64;
    eprintln!(
        "  delta vs prior: {} (added {}, removed {})",
        net, delta.added, delta.removed
    );
    if !delta.by_check.is_empty() {
        eprintln!("  by check:");
        // Sort by absolute magnitude descending; ties alphabetical.
        let mut sorted: Vec<(&String, &i64)> = delta.by_check.iter().collect();
        sorted.sort_by(|(a_id, a_val), (b_id, b_val)| {
            b_val
                .unsigned_abs()
                .cmp(&a_val.unsigned_abs())
                .then_with(|| a_id.cmp(b_id))
        });
        let tail = sorted.len().saturating_sub(BY_CHECK_LIMIT);
        for (id, val) in sorted.iter().take(BY_CHECK_LIMIT) {
            eprintln!("    {:<40} {:+}", id, val);
        }
        if tail > 0 {
            eprintln!("    ... and {} more", tail);
        }
    }
}

fn render_text(delta: &Delta) {
    // Text mode: print to stdout.
    let net: i64 = delta.added as i64 - delta.removed as i64;
    println!(
        "delta vs prior: {} (added {}, removed {})",
        net, delta.added, delta.removed
    );
    if !delta.by_check.is_empty() {
        println!("by check:");
        let mut sorted: Vec<(&String, &i64)> = delta.by_check.iter().collect();
        sorted.sort_by(|(a_id, a_val), (b_id, b_val)| {
            b_val
                .unsigned_abs()
                .cmp(&a_val.unsigned_abs())
                .then_with(|| a_id.cmp(b_id))
        });
        let tail = sorted.len().saturating_sub(BY_CHECK_LIMIT);
        for (id, val) in sorted.iter().take(BY_CHECK_LIMIT) {
            println!("  {:<40} {:+}", id, val);
        }
        if tail > 0 {
            println!("  ... and {} more", tail);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load(path: &Path) -> Result<Baseline, String> {
    baseline::read(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_engine::baseline::{Baseline, BaselineEntry};
    use tempfile::TempDir;

    fn write_baseline_file(dir: &Path, name: &str, entries: Vec<BaselineEntry>) -> PathBuf {
        let path = dir.join(name);
        let bl = Baseline::new(entries);
        baseline::write(&path, &bl).unwrap();
        path
    }

    fn make_entry(check_id: &str, sig: &str) -> BaselineEntry {
        BaselineEntry {
            file: "src/a.ts".into(),
            check_id: check_id.into(),
            rule_signature: sig.into(),
        }
    }

    #[test]
    fn diff_two_files_matches_compute_delta() {
        let tmp = TempDir::new().unwrap();
        let prior_path = write_baseline_file(
            tmp.path(),
            "prior.json",
            vec![
                make_entry("Warning.TripleEquals", "sig1"),
                make_entry("Warning.TripleEquals", "sig2"),
            ],
        );
        let current_path = write_baseline_file(
            tmp.path(),
            "current.json",
            vec![make_entry("Warning.TripleEquals", "sig1")],
        );

        let prior = baseline::read(&prior_path).unwrap();
        let current = baseline::read(&current_path).unwrap();
        let expected = baseline::compute_delta(&prior, &current);

        let args = DiffArgs {
            a: Some(prior_path),
            b: Some(current_path),
            robot: false,
            pretty: false,
        };
        // We can't capture stdout easily in unit tests, so just verify it exits
        // successfully and the delta object we computed matches.
        let code = run(args);
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(expected.added, 0);
        assert_eq!(expected.removed, 1);
    }
}
