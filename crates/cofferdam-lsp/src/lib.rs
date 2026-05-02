//! `cofferdam-lsp` — Language Server Protocol implementation.
//!
//! Phase 6 deliverable. Will use `tower-lsp` to expose diagnostics, code
//! actions for autofixes, and `textDocument/codeAction` for severity
//! overrides. Editor integration is the long-tail adoption lever Credo
//! never had — its LSP shim parsed stdout, which broke every refactor.
