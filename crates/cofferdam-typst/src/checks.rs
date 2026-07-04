//! The 11 built-in Typst checks. One struct per check, registered in
//! `check::all_typst_checks()`.

use cofferdam_core::{span_from_bytes, Issue, Location, Severity, Span};

use crate::check::{priority_for_severity, TypstCheck, TypstCheckMeta};
use crate::package::TypstPackage;

/// Location for a manifest key. Falls back to line 1 (manifest not
/// parseable-with-spans, or the key doesn't exist) so every check still
/// has *somewhere* to point rather than panicking.
fn manifest_location(pkg: &TypstPackage, span: Option<Span>) -> Location {
    let span = span.unwrap_or(Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    });
    Location::from_span(&pkg.manifest_path, span)
}

/// Location for a byte range inside `README.md`.
fn readme_location(pkg: &TypstPackage, start: usize, end: usize) -> Location {
    let text = pkg.readme_text.as_deref().unwrap_or("");
    let span = span_from_bytes(text, start as u32, end as u32);
    let path = pkg
        .readme_path
        .clone()
        .unwrap_or_else(|| pkg.root.join("README.md"));
    Location::from_span(&path, span)
}

/// Location for a package-directory-level finding with no natural
/// in-file position (e.g. "LICENSE is missing"). Points at where the
/// file would live.
fn root_location(pkg: &TypstPackage, filename: &str) -> Location {
    let path = pkg.root.join(filename);
    Location::from_span(
        &path,
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        },
    )
}

fn issue(
    meta: &'static TypstCheckMeta,
    location: Location,
    file: std::path::PathBuf,
    message: String,
) -> Issue {
    Issue {
        check_id: meta.id.to_string(),
        message,
        file,
        location,
        priority: priority_for_severity(meta.default_severity),
        severity: meta.default_severity,
        related: Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Design.ManifestRequiredFields
// ---------------------------------------------------------------------

pub struct ManifestRequiredFields;

const MANIFEST_REQUIRED_FIELDS_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.ManifestRequiredFields",
    category: cofferdam_core::Category::Design,
    default_severity: Severity::High,
    explanation: "typst.toml [package] must declare name, version, entrypoint, authors, \
        license, and description — Universe rejects manifests missing any of these.",
};

impl TypstCheck for ManifestRequiredFields {
    fn meta(&self) -> &'static TypstCheckMeta {
        &MANIFEST_REQUIRED_FIELDS_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let m = &pkg.manifest;
        let mut missing = Vec::new();
        if m.name.is_none() {
            missing.push("name");
        }
        if m.version.is_none() {
            missing.push("version");
        }
        if m.entrypoint.is_none() {
            missing.push("entrypoint");
        }
        if m.authors.as_ref().is_none_or(Vec::is_empty) {
            missing.push("authors");
        }
        if m.license.is_none() {
            missing.push("license");
        }
        if m.description.is_none() {
            missing.push("description");
        }
        if missing.is_empty() {
            return Vec::new();
        }
        let meta = self.meta();
        vec![issue(
            meta,
            manifest_location(pkg, None),
            pkg.manifest_path.clone(),
            format!(
                "typst.toml [package] is missing required field(s): {}",
                missing.join(", ")
            ),
        )]
    }
}

// ---------------------------------------------------------------------
// Design.PackageNameNotCanonical
// ---------------------------------------------------------------------

pub struct PackageNameNotCanonical;

const PACKAGE_NAME_NOT_CANONICAL_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.PackageNameNotCanonical",
    category: cofferdam_core::Category::Design,
    default_severity: Severity::High,
    explanation: "Package name collides with a generic/reserved name already common in the \
        Typst Universe preview index. Pick something more specific.",
};

/// Starter blacklist of generic/reserved names, seeded from names already
/// common in https://github.com/typst/packages/tree/main/packages/preview.
/// Meant to be regenerated periodically rather than treated as exhaustive.
const CANONICAL_NAME_BLACKLIST: &[&str] = &[
    "calendar",
    "slides",
    "pdf",
    "book",
    "cv",
    "resume",
    "letter",
    "report",
    "thesis",
    "template",
    "table",
    "chart",
    "graph",
    "theme",
    "style",
    "poster",
    "invoice",
    "diagram",
    "presentation",
    "notes",
];

impl TypstCheck for PackageNameNotCanonical {
    fn meta(&self) -> &'static TypstCheckMeta {
        &PACKAGE_NAME_NOT_CANONICAL_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let Some(name) = pkg.manifest.name.as_deref() else {
            return Vec::new();
        };
        if !CANONICAL_NAME_BLACKLIST.contains(&name.to_ascii_lowercase().as_str()) {
            return Vec::new();
        }
        let meta = self.meta();
        vec![issue(
            meta,
            manifest_location(pkg, pkg.manifest_spans.name),
            pkg.manifest_path.clone(),
            format!("package name `{name}` is too generic — pick something more specific"),
        )]
    }
}

// ---------------------------------------------------------------------
// Design.PackageNameKebabCase
// ---------------------------------------------------------------------

pub struct PackageNameKebabCase;

const PACKAGE_NAME_KEBAB_CASE_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.PackageNameKebabCase",
    category: cofferdam_core::Category::Design,
    default_severity: Severity::High,
    explanation: "Package names must be lowercase kebab-case (no underscores, no uppercase) \
        and must not contain the substring `typst`.",
};

fn is_kebab_case(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut prev_was_dash = true; // reject leading dash
    for c in name.chars() {
        if c == '-' {
            if prev_was_dash {
                return false; // leading or double dash
            }
            prev_was_dash = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_was_dash = false;
        } else {
            return false;
        }
    }
    !prev_was_dash // reject trailing dash
}

impl TypstCheck for PackageNameKebabCase {
    fn meta(&self) -> &'static TypstCheckMeta {
        &PACKAGE_NAME_KEBAB_CASE_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let Some(name) = pkg.manifest.name.as_deref() else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        if !is_kebab_case(name) {
            problems.push("must be lowercase kebab-case (letters, digits, single hyphens)");
        }
        if name.to_ascii_lowercase().contains("typst") {
            problems.push("must not contain `typst`");
        }
        if problems.is_empty() {
            return Vec::new();
        }
        let meta = self.meta();
        vec![issue(
            meta,
            manifest_location(pkg, pkg.manifest_spans.name),
            pkg.manifest_path.clone(),
            format!("package name `{name}` {}", problems.join("; ")),
        )]
    }
}

// ---------------------------------------------------------------------
// Readability.DescriptionStyle
// ---------------------------------------------------------------------

pub struct DescriptionStyle;

const DESCRIPTION_STYLE_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.DescriptionStyle",
    category: cofferdam_core::Category::Readability,
    default_severity: Severity::Low,
    explanation: "Description should end with a period, avoid a leading `A `/`An `, avoid \
        redundantly saying package/template/Typst, and land around 40-60 characters.",
};

impl TypstCheck for DescriptionStyle {
    fn meta(&self) -> &'static TypstCheckMeta {
        &DESCRIPTION_STYLE_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let Some(desc) = pkg.manifest.description.as_deref() else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        if !desc.ends_with('.') {
            problems.push("should end with a period".to_string());
        }
        if desc.starts_with("A ") || desc.starts_with("An ") {
            problems.push("should not start with `A `/`An `".to_string());
        }
        for word in ["package", "template", "Typst"] {
            if desc.contains(word) {
                problems.push(format!("should not contain `{word}`"));
            }
        }
        let len = desc.chars().count();
        if !(40..=60).contains(&len) {
            problems.push(format!("ideally 40-60 characters (got {len})"));
        }
        if problems.is_empty() {
            return Vec::new();
        }
        let meta = self.meta();
        vec![issue(
            meta,
            manifest_location(pkg, pkg.manifest_spans.description),
            pkg.manifest_path.clone(),
            format!("description style: {}", problems.join("; ")),
        )]
    }
}

// ---------------------------------------------------------------------
// Consistency.ManifestVersionMatchesDir
// ---------------------------------------------------------------------

pub struct ManifestVersionMatchesDir;

const MANIFEST_VERSION_MATCHES_DIR_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.ManifestVersionMatchesDir",
    category: cofferdam_core::Category::Consistency,
    default_severity: Severity::High,
    explanation: "When a package sits under `preview/<name>/<version>/`, the manifest \
        `version` must match the version directory name.",
};

impl TypstCheck for ManifestVersionMatchesDir {
    fn meta(&self) -> &'static TypstCheckMeta {
        &MANIFEST_VERSION_MATCHES_DIR_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let Some(dir_version) = pkg.version_dir.as_deref() else {
            return Vec::new();
        };
        let Some(manifest_version) = pkg.manifest.version.as_deref() else {
            return Vec::new();
        };
        if manifest_version == dir_version {
            return Vec::new();
        }
        let meta = self.meta();
        vec![issue(
            meta,
            manifest_location(pkg, pkg.manifest_spans.version),
            pkg.manifest_path.clone(),
            format!(
                "manifest version `{manifest_version}` does not match version directory `{dir_version}`"
            ),
        )]
    }
}

// ---------------------------------------------------------------------
// Consistency.ReadmeVersionMatchesManifest
// ---------------------------------------------------------------------

pub struct ReadmeVersionMatchesManifest;

const README_VERSION_MATCHES_MANIFEST_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.ReadmeVersionMatchesManifest",
    category: cofferdam_core::Category::Consistency,
    default_severity: Severity::Medium,
    explanation: "`@preview/<name>:X.Y.Z` import examples in README.md must match the \
        manifest version.",
};

/// Find every `@preview/<name>:<version>` occurrence in `text`, returning
/// `(version_str, start_byte, end_byte)` for the version portion.
fn find_preview_imports<'a>(text: &'a str, name: &str) -> Vec<(&'a str, usize, usize)> {
    let prefix = format!("@preview/{name}:");
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find(&prefix) {
        let version_start = search_from + rel + prefix.len();
        let rest = &text[version_start..];
        let version_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let version_end = version_start + version_len;
        if version_len > 0 {
            out.push((
                &text[version_start..version_end],
                version_start,
                version_end,
            ));
        }
        search_from = version_end.max(version_start + 1);
    }
    out
}

impl TypstCheck for ReadmeVersionMatchesManifest {
    fn meta(&self) -> &'static TypstCheckMeta {
        &README_VERSION_MATCHES_MANIFEST_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let (Some(readme), Some(name), Some(manifest_version)) = (
            pkg.readme_text.as_deref(),
            pkg.manifest.name.as_deref(),
            pkg.manifest.version.as_deref(),
        ) else {
            return Vec::new();
        };
        let meta = self.meta();
        find_preview_imports(readme, name)
            .into_iter()
            .filter(|(found_version, ..)| *found_version != manifest_version)
            .map(|(found_version, start, end)| {
                issue(
                    meta,
                    readme_location(pkg, start, end),
                    pkg.readme_path.clone().unwrap_or_else(|| pkg.root.join("README.md")),
                    format!(
                        "README import uses version `{found_version}`, manifest declares `{manifest_version}`"
                    ),
                )
            })
            .collect()
    }
}

// ---------------------------------------------------------------------
// Consistency.LicenseFileMatchesSPDX
// ---------------------------------------------------------------------

pub struct LicenseFileMatchesSpdx;

const LICENSE_FILE_MATCHES_SPDX_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.LicenseFileMatchesSPDX",
    category: cofferdam_core::Category::Consistency,
    default_severity: Severity::Medium,
    explanation: "A LICENSE file must exist and the manifest `license` field must be a \
        recognised SPDX identifier. v1 checks existence + known-id only, not license text \
        content.",
};

/// Small baked-in set of common OSS SPDX identifiers. Not exhaustive —
/// widen as real submissions hit gaps.
const KNOWN_SPDX_IDS: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "MPL-2.0",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "LGPL-2.1-only",
    "LGPL-3.0-only",
    "AGPL-3.0-only",
    "ISC",
    "Unlicense",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "EUPL-1.2",
    "Zlib",
    "0BSD",
];

impl TypstCheck for LicenseFileMatchesSpdx {
    fn meta(&self) -> &'static TypstCheckMeta {
        &LICENSE_FILE_MATCHES_SPDX_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let license = pkg.manifest.license.as_deref();
        let known_id = license.is_some_and(|l| KNOWN_SPDX_IDS.contains(&l));
        if known_id && pkg.license_path.is_some() {
            return Vec::new();
        }
        let meta = self.meta();
        let message = match (license, pkg.license_path.is_some()) {
            (None, _) => "manifest `license` field is missing".to_string(),
            (Some(l), false) => format!("LICENSE file is missing (manifest declares `{l}`)"),
            (Some(l), true) => format!("`{l}` is not a recognised SPDX identifier"),
        };
        vec![issue(
            meta,
            manifest_location(pkg, pkg.manifest_spans.license),
            pkg.manifest_path.clone(),
            message,
        )]
    }
}

// ---------------------------------------------------------------------
// Refactor.RelativeImportInPublishedReadme
// ---------------------------------------------------------------------

pub struct RelativeImportInPublishedReadme;

const RELATIVE_IMPORT_IN_PUBLISHED_README_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.RelativeImportInPublishedReadme",
    category: cofferdam_core::Category::Refactor,
    default_severity: Severity::Medium,
    explanation: "README examples should import via `@preview/<name>:<version>` (works for \
        users installing the package), not a relative path like `../lib.typ` (only works \
        inside the source repo).",
};

impl TypstCheck for RelativeImportInPublishedReadme {
    fn meta(&self) -> &'static TypstCheckMeta {
        &RELATIVE_IMPORT_IN_PUBLISHED_README_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        let Some(readme) = pkg.readme_text.as_deref() else {
            return Vec::new();
        };
        let meta = self.meta();
        let mut out = Vec::new();
        for needle in ["#import \"../", "#import \"./"] {
            let mut search_from = 0;
            while let Some(rel) = readme[search_from..].find(needle) {
                let start = search_from + rel;
                let after_quote = start + needle.len();
                let end = readme[after_quote..]
                    .find('"')
                    .map(|p| after_quote + p)
                    .unwrap_or_else(|| readme.len().min(after_quote + 40));
                out.push(issue(
                    meta,
                    readme_location(pkg, start, end),
                    pkg.readme_path.clone().unwrap_or_else(|| pkg.root.join("README.md")),
                    "README example imports via a relative path — use `@preview/<name>:<version>` instead".to_string(),
                ));
                search_from = start + needle.len();
            }
        }
        out
    }
}

// ---------------------------------------------------------------------
// Warning.BundleIncludesPdf
// ---------------------------------------------------------------------

pub struct BundleIncludesPdf;

const BUNDLE_INCLUDES_PDF_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.BundleIncludesPdf",
    category: cofferdam_core::Category::Warning,
    default_severity: Severity::Medium,
    explanation: "Root-level PDF files bloat the published bundle unless explicitly excluded \
        via the manifest `exclude` array.",
};

impl TypstCheck for BundleIncludesPdf {
    fn meta(&self) -> &'static TypstCheckMeta {
        &BUNDLE_INCLUDES_PDF_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        if pkg.pdf_files.is_empty() {
            return Vec::new();
        }
        let exclude = pkg.manifest.exclude.clone().unwrap_or_default();
        let meta = self.meta();
        pkg.pdf_files
            .iter()
            .filter(|pdf| {
                let name = pdf.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !exclude
                    .iter()
                    .any(|e| e == name || e.trim_start_matches("./") == name)
            })
            .map(|pdf| {
                let name = pdf.file_name().and_then(|n| n.to_str()).unwrap_or("");
                issue(
                    meta,
                    Location::from_span(
                        pdf,
                        Span {
                            start_byte: 0,
                            end_byte: 0,
                            line: 1,
                            column: 1,
                        },
                    ),
                    pdf.clone(),
                    format!("`{name}` ships in the bundle but is not listed in [package].exclude"),
                )
            })
            .collect()
    }
}

// ---------------------------------------------------------------------
// Warning.LicenseMissing
// ---------------------------------------------------------------------

pub struct LicenseMissing;

const LICENSE_MISSING_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.LicenseMissing",
    category: cofferdam_core::Category::Warning,
    default_severity: Severity::High,
    explanation: "No LICENSE file in the package directory — Universe requires one.",
};

impl TypstCheck for LicenseMissing {
    fn meta(&self) -> &'static TypstCheckMeta {
        &LICENSE_MISSING_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        if pkg.license_path.is_some() {
            return Vec::new();
        }
        let meta = self.meta();
        let loc = root_location(pkg, "LICENSE");
        vec![issue(
            meta,
            loc,
            pkg.root.join("LICENSE"),
            "package directory has no LICENSE file".to_string(),
        )]
    }
}

// ---------------------------------------------------------------------
// Warning.ChangelogMissing
// ---------------------------------------------------------------------

pub struct ChangelogMissing;

const CHANGELOG_MISSING_META: TypstCheckMeta = TypstCheckMeta {
    id: "Typst.ChangelogMissing",
    category: cofferdam_core::Category::Warning,
    default_severity: Severity::Low,
    explanation: "No CHANGELOG.md in the package directory. Not a hard Universe requirement, \
        but commonly requested by reviewers.",
};

impl TypstCheck for ChangelogMissing {
    fn meta(&self) -> &'static TypstCheckMeta {
        &CHANGELOG_MISSING_META
    }

    fn check(&self, pkg: &TypstPackage) -> Vec<Issue> {
        if pkg.changelog_path.is_some() {
            return Vec::new();
        }
        let meta = self.meta();
        let loc = root_location(pkg, "CHANGELOG.md");
        vec![issue(
            meta,
            loc,
            pkg.root.join("CHANGELOG.md"),
            "package directory has no CHANGELOG.md".to_string(),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_case_accepts_valid_names() {
        assert!(is_kebab_case("my-cool-pkg"));
        assert!(is_kebab_case("pkg"));
        assert!(is_kebab_case("a1-b2"));
    }

    #[test]
    fn kebab_case_rejects_invalid_names() {
        assert!(!is_kebab_case("My-Pkg"));
        assert!(!is_kebab_case("my_pkg"));
        assert!(!is_kebab_case("-leading"));
        assert!(!is_kebab_case("trailing-"));
        assert!(!is_kebab_case("double--dash"));
        assert!(!is_kebab_case(""));
    }

    #[test]
    fn find_preview_imports_extracts_version() {
        let text = "Install with `@preview/foo:1.2.3` in your document.";
        let found = find_preview_imports(text, "foo");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "1.2.3");
    }

    #[test]
    fn find_preview_imports_ignores_other_package_names() {
        let text = "@preview/bar:1.0.0";
        assert!(find_preview_imports(text, "foo").is_empty());
    }
}
