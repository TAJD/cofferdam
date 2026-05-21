//! `cofferdam watch` — re-analyze on file change (cd-9hp.4 cp1b).
//!
//! Daemon loop on top of [`cofferdam_engine::cache::ParseCache`].
//! Discovers TS files under the requested roots, registers a
//! recursive filesystem watcher on each, and re-runs the engine
//! whenever a change drains the debounce window. The shared cache
//! survives across iterations so unchanged files skip parse on the
//! next pass.
//!
//! ## What watch deliberately does NOT do
//!
//! - **Baselines, suppression, `--since`, output formats other than
//!   text.** Watch is a dev-loop affordance, not a CI surface; the
//!   richer flags belong on `cofferdam check`.
//! - **Severity gating / exit codes.** The loop runs until Ctrl-C;
//!   the only non-zero exit is on setup error (config load, watcher
//!   creation).
//! - **Plugin invocation.** Plugins are stateful and not yet wired
//!   through the cache; we'll lift them in alongside cp2's findings
//!   cache.
//!
//! ## Debounce
//!
//! Notify emits one event per file system mutation (some platforms
//! amplify a single editor save into multiple write/create/rename
//! events). We collect events on an mpsc channel, then drain with a
//! `recv_timeout(debounce)` pattern: once the first event arrives,
//! keep draining until the channel stays silent for the configured
//! window, then re-run. Typical editor saves settle in 50–150 ms;
//! the default `--debounce 100` covers the common case without
//! adding visible latency.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use cofferdam_checks::all_builtins;
use cofferdam_core::Issue;
use cofferdam_engine::cache::ParseCache;
use cofferdam_engine::{config::resolve_with_invariants, discover, DiscoveryOptions, Engine};
use cofferdam_formatters::TextFormatter;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// CLI args for `cofferdam watch`.
pub struct WatchArgs {
    pub paths: Vec<PathBuf>,
    pub hidden: bool,
    pub no_ignore: bool,
    pub config_path: Option<PathBuf>,
    pub no_config: bool,
    pub debounce_ms: u64,
}

/// Entry point invoked from `main`.
pub fn run(args: WatchArgs) -> ExitCode {
    let WatchArgs {
        paths,
        hidden,
        no_ignore,
        config_path,
        no_config,
        debounce_ms,
    } = args;

    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let absolute_roots: Vec<PathBuf> = roots
        .iter()
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();

    let discovery_opts = DiscoveryOptions {
        respect_ignore: !no_ignore,
        include_hidden: hidden,
        ..DiscoveryOptions::default()
    };

    let engine = match build_engine(config_path.as_deref(), no_config) {
        Ok(e) => e,
        Err(()) => return ExitCode::from(2),
    };

    // Same cache across every iteration — the whole point of the
    // exercise. Survives until the process is killed.
    let cache = ParseCache::new();

    // First pass: analyze whatever's there now so the user sees a
    // baseline of findings before any keystroke.
    let initial_files = match discover(&roots, &discovery_opts) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    print_separator("initial");
    let initial_issues = analyze_once(&engine, &cache, &initial_files);
    print_findings(&initial_issues);

    // Filesystem watcher. Recursive on every root so subtree changes
    // bubble up. The `recommended_watcher` constructor picks
    // ReadDirectoryChangesW on Windows, FSEvents on macOS, inotify
    // on Linux — all native, no polling fallback unless the kernel
    // backend errors.
    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher: RecommendedWatcher = match Watcher::new(tx, NotifyConfig::default()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: failed to create filesystem watcher: {e}");
            return ExitCode::from(2);
        }
    };
    for root in &absolute_roots {
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            eprintln!("error: failed to watch `{}`: {e}", root.display());
            return ExitCode::from(2);
        }
    }

    if !no_config {
        eprintln!(
            "cofferdam watch: monitoring {} (Ctrl-C to stop)",
            absolute_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let debounce = Duration::from_millis(debounce_ms.max(1));
    loop {
        // Block until the next event, then drain anything that
        // arrives within `debounce` of the previous event. Errors
        // from the watcher channel are surfaced but do not kill the
        // loop — a transient inotify hiccup shouldn't take the dev
        // affordance down.
        match drain_events(&rx, debounce) {
            DrainOutcome::Idle => continue,
            DrainOutcome::Disconnected => {
                eprintln!("cofferdam watch: watcher channel closed; exiting");
                return ExitCode::SUCCESS;
            }
            DrainOutcome::Changed => {
                let files = match discover(&roots, &discovery_opts) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("error: {e}");
                        continue;
                    }
                };
                print_separator("change");
                let issues = analyze_once(&engine, &cache, &files);
                print_findings(&issues);
            }
        }
    }
}

fn build_engine(config_path: Option<&Path>, no_config: bool) -> Result<Engine, ()> {
    if no_config {
        return Ok(Engine::new(all_builtins()));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (project_config, resolved_path, diags) =
        match resolve_with_invariants(config_path, &cwd, false) {
            Ok(triple) => triple,
            Err(e) => {
                eprintln!("error: {e}");
                return Err(());
            }
        };
    for warn in &diags.warnings {
        eprintln!("warning: {warn}");
    }
    let engine = match project_config.as_ref() {
        Some(cfg) => {
            let path = resolved_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            match Engine::with_config(all_builtins(), cfg, path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("error: {e}");
                    return Err(());
                }
            }
        }
        None => Engine::new(all_builtins()),
    };
    Ok(engine)
}

/// Read every discovered file from disk, run the engine through the
/// cache, and apply severity overrides (mirrors `run_check`'s
/// post-analyze stamping). Returns the issue list ready for rendering.
fn analyze_once(engine: &Engine, cache: &ParseCache, files: &[PathBuf]) -> Vec<Issue> {
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(text) => sources.push((path.clone(), text)),
            Err(e) => {
                eprintln!("warning: skipping `{}`: {e}", path.display());
            }
        }
    }
    let (issues, _texts) = engine.analyze_with_sources_cached(sources, Some(cache));
    issues
}

fn print_findings(issues: &[Issue]) {
    // TextFormatter::render emits the "N finding(s)" summary line
    // itself, so we just hand off. Watch mode's per-iteration banner
    // already names the run; an extra footer line would be noise.
    print!("{}", TextFormatter::render(issues));
}

fn print_separator(label: &str) {
    let ts = chrono_like_now();
    println!();
    println!("─── {label} · {ts} ──────────────");
}

/// Render a timestamp the way humans read terminal output. We don't
/// pull in `chrono` for this — `SystemTime::now()` plus a quick
/// formatter is enough for a watch banner.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // HH:MM:SS in UTC. Good enough for a relative dev-loop banner.
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}

enum DrainOutcome {
    /// Channel was idle for the full debounce window — no action.
    Idle,
    /// At least one filesystem event landed; analyzer should re-run.
    Changed,
    /// Watcher disconnected (sender dropped). Caller should exit.
    Disconnected,
}

/// Block until the first relevant event, then absorb a
/// quiet-window's worth of follow-up events before yielding
/// `Changed`. Trailing-debounce semantics: every new event resets
/// the deadline, so a flurry of saves collapses to a single
/// `Changed` once the channel stays silent for `debounce`.
///
/// Returns `Idle` when the first event was filtered as irrelevant
/// — the caller re-enters this function to wait for the next one.
fn drain_events(
    rx: &std::sync::mpsc::Receiver<notify::Result<Event>>,
    debounce: Duration,
) -> DrainOutcome {
    // The initial recv is unbounded — we wait as long as it takes
    // for the first event. recv_timeout would force a busy-loop
    // for the no-activity case, which is the wrong tradeoff.
    let first = match rx.recv() {
        Ok(ev) => ev,
        Err(_) => return DrainOutcome::Disconnected,
    };
    if !event_is_relevant(&first) {
        return DrainOutcome::Idle;
    }

    let mut deadline = Instant::now() + debounce;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return DrainOutcome::Changed;
        }
        match rx.recv_timeout(remaining) {
            Ok(_ev) => {
                // Trailing debounce: every event resets the window.
                deadline = Instant::now() + debounce;
            }
            Err(RecvTimeoutError::Timeout) => return DrainOutcome::Changed,
            Err(RecvTimeoutError::Disconnected) => return DrainOutcome::Disconnected,
        }
    }
}

/// Filter watcher events to the subset we care about. Notify
/// reports access / metadata events on some platforms (touch
/// timestamps, attribute changes) that don't move the AST needle.
/// We want create/modify/remove/rename only.
fn event_is_relevant(result: &notify::Result<Event>) -> bool {
    use notify::EventKind;
    match result {
        Ok(ev) => matches!(
            ev.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ),
        // Surface watcher errors but don't take the loop down — the
        // next event recovers most transient backend issues.
        Err(e) => {
            eprintln!("cofferdam watch: notify error: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn analyze_once_uses_the_shared_cache() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("widget.ts");
        fs::write(&path, "export const x = 1;\n").expect("write");

        let engine = Engine::new(all_builtins());
        let cache = ParseCache::new();
        let _ = analyze_once(&engine, &cache, std::slice::from_ref(&path));
        let misses = cache.misses();
        assert!(misses >= 1, "first pass must record a parse miss");

        let _ = analyze_once(&engine, &cache, std::slice::from_ref(&path));
        assert_eq!(
            cache.misses(),
            misses,
            "second pass against unchanged text must add no misses"
        );
        assert!(cache.hits() >= 1, "second pass must register a cache hit");
    }

    #[test]
    fn modifying_a_file_re_misses_the_cache() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("widget.ts");
        fs::write(&path, "export const x = 1;\n").expect("write");

        let engine = Engine::new(all_builtins());
        let cache = ParseCache::new();
        let _ = analyze_once(&engine, &cache, std::slice::from_ref(&path));
        let misses_after_first = cache.misses();

        // Rewrite with different bytes — the content hash changes,
        // so the next pass misses again.
        fs::write(&path, "export const x = 2;\n").expect("rewrite");
        let _ = analyze_once(&engine, &cache, std::slice::from_ref(&path));
        assert_eq!(
            cache.misses(),
            misses_after_first + 1,
            "edited file must record exactly one new parse miss"
        );
    }

    #[test]
    fn event_is_relevant_filters_metadata_noise() {
        use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};
        use notify::EventKind;
        let mk = |kind| {
            Ok::<Event, notify::Error>(Event {
                kind,
                paths: vec![],
                attrs: Default::default(),
            })
        };
        assert!(event_is_relevant(&mk(EventKind::Create(CreateKind::File))));
        assert!(event_is_relevant(&mk(EventKind::Modify(ModifyKind::Any))));
        assert!(event_is_relevant(&mk(EventKind::Remove(RemoveKind::File))));
        // Metadata-only access events don't trigger a re-run.
        assert!(!event_is_relevant(&mk(EventKind::Access(AccessKind::Read))));
    }
}
