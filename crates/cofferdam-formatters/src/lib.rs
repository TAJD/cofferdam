//! `cofferdam-formatters` — pluggable report renderers.
//!
//! Ships text (humans), JSON (CI / tools), compact (AI agents), and
//! SARIF 2.1.0 (GitHub Code Scanning + every other static-analysis
//! consumer). The GitHub-annotation format is still planned.

pub mod compact;
pub mod json;
pub mod sarif;
pub mod text;

pub use compact::{CompactFormatter, COMPACT_HEADER};
pub use json::{JsonFormatter, JsonRenderOpts};
pub use sarif::{SarifFormatter, SarifRenderOpts};
pub use text::{TextFormatter, TextRenderOpts};
