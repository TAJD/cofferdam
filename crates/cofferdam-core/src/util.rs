//! Small shared utilities used across the cofferdam crate family.

use std::path::Path;

/// Build a normalized path key by lowercasing on Windows (case-insensitive
/// filesystem) and using as-is on case-sensitive platforms. Avoids spurious
/// mismatches when oxc_resolver returns `C:\Foo` and discovery returned
/// `C:\foo`.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// Heuristic: a token is plausibly a `Category.Name`-style check id if
/// it contains at least one dot, starts with an ASCII letter, and only
/// uses ASCII identifier chars / dots / underscores. Conservative —
/// avoids binding random English words as ids.
pub fn looks_like_check_id(s: &str) -> bool {
    if s.is_empty() || !s.contains('.') {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_key_forward_slashes() {
        let p = Path::new("a/b/c.ts");
        assert_eq!(path_key(p), "a/b/c.ts");
    }

    #[cfg(windows)]
    #[test]
    fn path_key_lowercased_on_windows() {
        let p = Path::new("C:\\Users\\Foo\\bar.ts");
        let k = path_key(p);
        assert!(
            k.chars().all(|c| !c.is_ascii_uppercase()),
            "expected lowercase: {k}"
        );
        assert!(k.contains('/'), "expected forward slashes: {k}");
    }

    #[test]
    fn looks_like_check_id_valid() {
        assert!(looks_like_check_id("Design.OrphanExport"));
        assert!(looks_like_check_id("Warning.TripleEquals"));
        assert!(looks_like_check_id("Plugin.Custom.Subcheck"));
    }

    #[test]
    fn looks_like_check_id_invalid() {
        assert!(!looks_like_check_id("please"));
        assert!(!looks_like_check_id(""));
        assert!(!looks_like_check_id("123.bad"));
        assert!(!looks_like_check_id("no-dashes-allowed.x"));
        assert!(!looks_like_check_id("Just_underscores"));
    }
}
