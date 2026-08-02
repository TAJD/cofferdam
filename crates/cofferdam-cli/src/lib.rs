//! Library surface for the `cofferdam` CLI's subcommand logic.
//!
//! Exists so `cofferdam-mcp` can call the same advise/explain/diff
//! projection code the CLI binary uses, instead of re-implementing it
//! (CD-60). `main.rs` is a thin binary that parses args and delegates
//! into these modules.

pub mod advise;
pub mod advise_diff;
pub mod agents;
pub mod ast_wire;
pub mod baseline_diff;
pub mod baseline_lint;
pub mod context_cmd;
pub mod context_digest;
pub mod doctor;
pub mod explain;
pub mod gen_docs;
pub mod html_wire;
pub mod init;
pub mod invariants_tool;
pub mod plugins;
pub mod type_host;
pub mod watch;
