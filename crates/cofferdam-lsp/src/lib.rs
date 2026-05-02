//! `cofferdam-lsp` — Language Server Protocol implementation.
//!
//! Phase 6 deliverable. Will use `tower-lsp` to expose diagnostics, code
//! actions for autofixes, and `textDocument/codeAction` for severity
//! overrides. Editor integration is the long-tail adoption lever — many
//! Elixir/JS analyzers ship LSP shims that parse stdout, which breaks on
//! every output change. cofferdam exposes diagnostics natively over LSP.
