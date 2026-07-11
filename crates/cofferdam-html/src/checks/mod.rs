//! Per-check modules for the HTML adapter (CD-83).
//!
//! Each check ships in its own file and is registered via
//! [`all_html_checks`], mirroring `cofferdam-rust::checks`.

pub mod missing_lang_attribute;

use cofferdam_core::Check;

/// Every HTML adapter check that should run on a `Language::Html`
/// source file. `cofferdam-checks::all_builtins()` extends its
/// registry with this set.
pub fn all_html_checks() -> Vec<Box<dyn Check>> {
    vec![Box::new(missing_lang_attribute::MissingLangAttribute)]
}
