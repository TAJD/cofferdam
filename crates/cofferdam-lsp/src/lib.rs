//! `cofferdam-lsp` — Language Server Protocol implementation (cd-9hp.4 cp5).
//!
//! Built on `lsp-server` (rust-analyzer's sync transport library) +
//! `lsp-types` (protocol message definitions). Engine is sync, so
//! pulling tokio would buy nothing — `lsp-server` runs a plain
//! receive loop on a dedicated thread.
//!
//! ## What ships
//!
//! - Workspace-aware diagnostics. On `initialize` the server walks
//!   the workspace root with the engine's `discover` module, runs the
//!   full engine, and publishes per-file diagnostics. Cross-file
//!   checks (orphan exports, layer violations, duplicate exports)
//!   participate exactly as they do in `cofferdam check` — no
//!   single-file approximations.
//!
//! - Disk-cache warmth. The cp4 [`cofferdam_engine::disk_cache`] is
//!   hydrated at startup and saved at shutdown. Re-runs after the
//!   initial scan reuse the cp2 / cp3 in-memory caches plus the cp4
//!   on-disk persistence.
//!
//! - Save-time re-analysis. `textDocument/didSave` triggers a full
//!   workspace re-analyze with cache reuse. The cp3 run-cache lifts
//!   no-op re-runs to single-digit ms. `didChange` is intentionally
//!   not wired in cp5 — that's the live-keystroke path and would
//!   need a debouncer + worker thread separate from the receive
//!   loop. Save-only fits the bead's "editor shows findings on file
//!   save within ~500ms" gate.
//!
//! ## What is deferred
//!
//! - Code actions / autofixes — separate beads.
//! - `textDocument/diagnostic` pull model (3.17+) — push-only for now.
//!   Push is universally supported and matches our re-analysis cadence.
//! - Incremental text sync. Full sync is simpler and the engine doesn't
//!   benefit from byte-range updates anyway.

pub mod diagnostic;
pub mod server;
pub mod state;

pub use server::{run_server, run_stdio_server};
