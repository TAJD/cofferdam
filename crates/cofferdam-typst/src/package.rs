//! `TypstPackage` — the unit of analysis. One package directory, loaded
//! once, handed to every `TypstCheck`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifest::{parse_manifest, parse_manifest_spans, Manifest, ManifestSpans};

/// A loaded Typst package directory: the parsed manifest plus the
/// presence/paths of the other files Universe submission cares about.
pub struct TypstPackage {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_text: String,
    pub manifest: Manifest,
    pub manifest_spans: ManifestSpans,
    /// The `<version>` directory name when `root` sits under
    /// `…/preview/<name>/<version>/`. `None` for packages checked
    /// outside that layout (e.g. a package being authored locally,
    /// not yet placed under a `preview/` tree).
    pub version_dir: Option<String>,
    pub license_path: Option<PathBuf>,
    pub readme_path: Option<PathBuf>,
    pub readme_text: Option<String>,
    pub changelog_path: Option<PathBuf>,
    /// Root-level `*.pdf` files (not recursive — Universe bundle hygiene
    /// cares about what ships at the package root).
    pub pdf_files: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
}

/// Load a `TypstPackage` from `root`. Fails only if `root/typst.toml`
/// can't be read or doesn't parse as TOML — every other file is
/// optional and its absence is exactly what several checks flag.
pub fn load(root: &Path) -> Result<TypstPackage, LoadError> {
    let manifest_path = root.join("typst.toml");
    let manifest_text = fs::read_to_string(&manifest_path).map_err(|source| LoadError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = parse_manifest(&manifest_text).map_err(|source| LoadError::Toml {
        path: manifest_path.clone(),
        source: Box::new(source),
    })?;
    let manifest_spans = parse_manifest_spans(&manifest_text);

    let license_path = existing_file(&root.join("LICENSE"));
    let readme_path = existing_file(&root.join("README.md"));
    let readme_text = readme_path
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok());
    let changelog_path = existing_file(&root.join("CHANGELOG.md"));
    let pdf_files = collect_root_pdfs(root);
    let version_dir = detect_version_dir(root);

    Ok(TypstPackage {
        root: root.to_path_buf(),
        manifest_path,
        manifest_text,
        manifest,
        manifest_spans,
        version_dir,
        license_path,
        readme_path,
        readme_text,
        changelog_path,
        pdf_files,
    })
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

fn collect_root_pdfs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut pdfs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        })
        .collect();
    pdfs.sort();
    pdfs
}

/// `root` sits under `…/preview/<name>/<version>/` when its parent's
/// parent is literally named `preview`. Returns `root`'s own directory
/// name (the version) in that case.
fn detect_version_dir(root: &Path) -> Option<String> {
    let name_dir = root.parent()?;
    let preview_dir = name_dir.parent()?;
    if preview_dir.file_name()?.to_str()? == "preview" {
        root.file_name()?.to_str().map(String::from)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_version_dir_under_preview_layout() {
        let root = Path::new("/repos/packages/preview/my-pkg/1.2.3");
        assert_eq!(detect_version_dir(root).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn no_version_dir_outside_preview_layout() {
        let root = Path::new("/repos/my-pkg");
        assert_eq!(detect_version_dir(root), None);
    }
}
