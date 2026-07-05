//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::path::Path;

mod boundary_frozen;
mod duplicate_export_name;
mod import_cycle;
mod invariant_violation;
mod layer_violation;
mod max_parameters;
mod orphan_export;
mod scripted_invariant;

pub use boundary_frozen::BoundaryFrozen;
pub use duplicate_export_name::DuplicateExportName;
pub use import_cycle::ImportCycle;
pub use invariant_violation::InvariantViolation;
pub use layer_violation::LayerViolation;
pub use max_parameters::MaxParameters;
pub use orphan_export::OrphanExport;
pub use scripted_invariant::ScriptedInvariant;

/// Normalise a path relative to the project root into forward-slash
/// separated form suitable for glob matching. Strip leading `./ ` and `/`
/// so glob authors don't have to anticipate them.
fn relative_normalised(project_root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(project_root).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    // Discovery sometimes hands us paths like `./src/foo.ts`; strip the
    // leading `./` so glob authors don't have to anticipate it. Also
    // strip a leading `/` for the rare case where strip_prefix fell
    // through and we got an absolute-looking string.
    s.trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ExportKind, ExportRecord, ImportKind, ImportRecord, ImportedName};
    use cofferdam_core::path_key;
    use cofferdam_core::public_api::{resolve_public_api, PublicApi};
    use cofferdam_core::Span;
    use std::path::PathBuf;

    // Helper: build a PublicApi from a slice of entry strings using
    // `project_root` as the resolved root, exercising `resolve_public_api`.
    fn make_public_api(entries: &[&str], project_root: &Path) -> PublicApi {
        let owned: Vec<String> = entries.iter().map(|s| s.to_string()).collect();
        resolve_public_api(&owned, project_root)
    }

    // Helper: normalise a path the same way the engine does for
    // `ExportRecord.file` — absolutize first (cd-q9f), then forward-slash
    // and lower-case on Windows (`path_key`). The absolutize step matches
    // what `Engine.analyze_with_sources` does to every input path, so
    // these tests mirror the real wire shape rather than the raw key()
    // form. cd-gro / gh #41 turned on the analogous absolutize in
    // resolve_public_api; key() must do the same on both sides or the
    // tests would diverge from production.
    fn key(p: &Path) -> String {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        path_key(&abs)
    }

    #[test]
    fn exact_path_matches() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/index.ts"], &root);
        let file_key = key(&root.join("src/index.ts"));
        assert!(api.is_match(&file_key), "exact path should match");
    }

    #[test]
    fn exact_path_no_spurious_match() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/index.ts"], &root);
        let other = key(&root.join("src/other.ts"));
        assert!(
            !api.is_match(&other),
            "unrelated file should not match exact entry"
        );
    }

    #[test]
    fn dot_slash_prefix_stripped() {
        // "./src/index.ts" must resolve the same as "src/index.ts".
        let root = PathBuf::from("/project");
        let api = make_public_api(&["./src/index.ts"], &root);
        let file_key = key(&root.join("src/index.ts"));
        assert!(
            api.is_match(&file_key),
            "./src/index.ts should strip ./ and match"
        );
    }

    #[test]
    fn glob_matches_multiple_files() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["components/ui/**/*.tsx"], &root);

        let button = key(&root.join("components/ui/button.tsx"));
        let nested = key(&root.join("components/ui/forms/input.tsx"));
        let outside = key(&root.join("components/other/widget.tsx"));

        assert!(
            api.is_match(&button),
            "glob should match direct child *.tsx"
        );
        assert!(api.is_match(&nested), "glob should match nested *.tsx");
        assert!(
            !api.is_match(&outside),
            "glob should not match outside the tree"
        );
    }

    #[test]
    fn glob_star_single_level() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/*.ts"], &root);

        let direct = key(&root.join("src/index.ts"));
        let nested = key(&root.join("src/sub/other.ts"));

        assert!(
            api.is_match(&direct),
            "single-level glob should match direct child"
        );
        assert!(
            !api.is_match(&nested),
            "single-level glob should not match nested path"
        );
    }

    #[test]
    fn empty_entries_matches_nothing() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&[], &root);
        let key = path_key(&root.join("src/anything.ts"));
        assert!(!api.is_match(&key), "empty allowlist should not match");
    }

    #[test]
    fn package_json_entry_is_ignored() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["package.json:exports"], &root);
        // No file key should match — the entry is silently dropped.
        let key = path_key(&root.join("src/index.ts"));
        assert!(!api.is_match(&key));
    }

    #[test]
    fn invalid_glob_does_not_panic() {
        // An unclosed bracket `[` is an invalid glob. resolve_public_api
        // must skip it rather than panicking.
        let root = PathBuf::from("/project");
        let api = make_public_api(&["src/[invalid"], &root);
        // Nothing should match, but also no panic.
        let key = path_key(&root.join("src/anything.ts"));
        assert!(!api.is_match(&key));
    }

    #[test]
    fn exact_and_glob_coexist() {
        let root = PathBuf::from("/project");
        // Mix: one exact path + one glob.
        let api = make_public_api(&["src/index.ts", "components/ui/**/*.tsx"], &root);

        let exact_key = key(&root.join("src/index.ts"));
        let glob_key = key(&root.join("components/ui/button.tsx"));
        let miss = key(&root.join("src/other.ts"));

        assert!(
            api.is_match(&exact_key),
            "exact entry should still match when globs present"
        );
        assert!(api.is_match(&glob_key), "glob entry should match");
        assert!(!api.is_match(&miss), "unrelated file should not match");
    }

    #[test]
    fn is_glob_pattern_detects_metacharacters() {
        use cofferdam_core::public_api::is_glob_pattern;
        assert!(is_glob_pattern("**/*.tsx"));
        assert!(is_glob_pattern("src/*.ts"));
        assert!(is_glob_pattern("src/[abc].ts"));
        assert!(is_glob_pattern("{a,b}.ts"));
        assert!(!is_glob_pattern("src/index.ts"));
        assert!(!is_glob_pattern("./src/index.ts"));
    }

    // cd-gro / gh #41: when the invariants spec is discovered from a
    // relative root, project_root is relative. The engine absolutizes
    // every source path via `std::path::absolute` (cd-q9f), so the
    // file_key passed to `is_match` is always absolute. Before the fix,
    // the exact entry stored a relative key and `exact.contains(absolute)`
    // silently missed every entry — the user's gh #41 symptom.
    //
    // Both halves are pinned here: exact-path lookup and the glob-path
    // root-prefix stripping that lets `apps/foo/**` match an absolute
    // `/cwd/apps/foo/x.ts`.
    #[test]
    fn relative_project_root_normalises_to_absolute_for_exact_match() {
        // Use the process CWD as the absolutize anchor — that's what
        // `std::path::absolute(".")` resolves against. The test asserts
        // the relative-project-root path produces the same exact key
        // as the absolute-project-root path would.
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from(".");
        let api = make_public_api(&["apps/web/src/App.tsx"], &rel_root);

        // Engine emits file paths as absolute (cd-q9f).
        let absolute_file = cwd.join("apps/web/src/App.tsx");
        let file_key = path_key(&absolute_file);
        assert!(
            api.is_match(&file_key),
            "exact entry with relative project_root must still match an absolute file_key — \
             cd-gro / gh #41. file_key={file_key}",
        );
    }

    #[test]
    fn relative_project_root_normalises_to_absolute_for_glob_match() {
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from(".");
        let api = make_public_api(&["apps/web/src/routes/**"], &rel_root);

        let absolute_file = cwd.join("apps/web/src/routes/index.tsx");
        let file_key = path_key(&absolute_file);
        assert!(
            api.is_match(&file_key),
            "glob entry with relative project_root must still match an absolute file_key — \
             cd-gro / gh #41. file_key={file_key}",
        );
    }

    #[test]
    fn dot_subdir_project_root_works_too() {
        // `./apps` style — relative but with a subdir segment. Also tests
        // that the absolutize step doesn't choke on a relative prefix.
        let cwd = std::env::current_dir().expect("cwd");
        let rel_root = PathBuf::from("./apps");
        let api = make_public_api(&["web/src/App.tsx"], &rel_root);

        let absolute_file = cwd.join("apps").join("web/src/App.tsx");
        let file_key = path_key(&absolute_file);
        assert!(api.is_match(&file_key));
    }

    // ── compute_orphans: re-export edge reachability (cd-klp) ──────────────

    fn span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        }
    }

    fn opts_default() -> orphan_export::OrphanOptions {
        orphan_export::OrphanOptions {
            include_type_only: false,
            test_patterns: Vec::new(),
            framework_entry_patterns: Vec::new(),
        }
    }

    fn named_export(file: &Path, name: &str) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::Named,
            type_only: false,
            span: span(),
            source_specifier: None,
            resolved_source: None,
        }
    }

    fn default_export(file: &Path) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: "default".to_string(),
            kind: ExportKind::Default,
            type_only: false,
            span: span(),
            source_specifier: None,
            resolved_source: None,
        }
    }

    fn reexport(file: &Path, name: &str, source: &Path) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::ReExport,
            type_only: false,
            span: span(),
            source_specifier: Some("./".to_string()),
            resolved_source: Some(source.to_path_buf()),
        }
    }

    fn named_import(
        from: &Path,
        resolved: &Path,
        source_name: &str,
        local_name: &str,
    ) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: source_name.to_string(),
                local_name: local_name.to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    fn default_import(from: &Path, resolved: &Path, local_name: &str) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: "default".to_string(),
                local_name: local_name.to_string(),
                kind: ImportKind::Default,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    fn namespace_import(from: &Path, resolved: &Path) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./".to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: vec![ImportedName {
                source_name: "*".to_string(),
                local_name: "ns".to_string(),
                kind: ImportKind::Namespace,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: span(),
        }
    }

    #[test]
    fn shadcn_barrel_consumed_via_named_reexport() {
        // dialog.tsx → index.ts (re-exports Dialog) → app.ts (imports Dialog).
        // The engine records the re-export as an ImportRecord with
        // resolved=dialog.tsx, source_name="Dialog", which puts
        // (dialog.tsx, "Dialog") in `touched`. dialog.tsx's Dialog
        // export must NOT be flagged as orphan.
        let dialog = PathBuf::from("/p/components/ui/dialog.tsx");
        let index = PathBuf::from("/p/components/ui/index.ts");
        let app = PathBuf::from("/p/app/page.tsx");

        let imports = vec![
            named_import(&index, &dialog, "Dialog", "Dialog"),
            named_import(&app, &index, "Dialog", "Dialog"),
        ];
        let exports = vec![
            named_export(&dialog, "Dialog"),
            reexport(&index, "Dialog", &dialog),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        assert!(
            issues.is_empty(),
            "Dialog should be reachable through the barrel; got: {:?}",
            issues
        );
    }

    #[test]
    fn aliased_named_reexport_consumed() {
        // export { Dialog as MyDialog } from './dialog' → engine records
        // an import with source_name="Dialog", local_name="MyDialog".
        // touched gets (dialog, "Dialog"), which matches dialog.tsx's
        // export name.
        let dialog = PathBuf::from("/p/dialog.tsx");
        let index = PathBuf::from("/p/index.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&index, &dialog, "Dialog", "MyDialog"),
            named_import(&app, &index, "MyDialog", "MyDialog"),
        ];
        let exports = vec![
            named_export(&dialog, "Dialog"),
            reexport(&index, "MyDialog", &dialog),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        assert!(
            issues.is_empty(),
            "Aliased re-export should reach the source name; got: {:?}",
            issues
        );
    }

    #[test]
    fn multi_level_named_reexport_consumed() {
        // primitive.ts → inner_barrel.ts (re-exports X) → outer_barrel.ts
        // (re-exports X) → app.ts (imports X). Each level contributes
        // its own per-edge ImportRecord, so primitive.ts:X stays
        // reachable through transitive barrels.
        let primitive = PathBuf::from("/p/primitive.ts");
        let inner = PathBuf::from("/p/inner.ts");
        let outer = PathBuf::from("/p/outer.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&inner, &primitive, "X", "X"),
            named_import(&outer, &inner, "X", "X"),
            named_import(&app, &outer, "X", "X"),
        ];
        let exports = vec![
            named_export(&primitive, "X"),
            reexport(&inner, "X", &primitive),
            reexport(&outer, "X", &inner),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        assert!(
            issues.is_empty(),
            "Multi-level barrels should chain; got: {:?}",
            issues
        );
    }

    #[test]
    fn dead_barrel_no_longer_shields_primitive() {
        // primitive.ts is re-exported from dead_barrel.ts but nobody
        // consumes dead_barrel.ts. Today's per-edge `touched` does
        // record (primitive, "PrimitiveX") because of the re-export
        // edge from dead_barrel — so primitive is NOT flagged. That's
        // the documented limit of this fix (out-of-scope: live-set
        // reachability filtering). The point of this test is to pin
        // the behavior so a future stricter implementation surfaces
        // here as an intentional change.
        let primitive = PathBuf::from("/p/primitive.ts");
        let barrel = PathBuf::from("/p/dead_barrel.ts");

        let imports = vec![named_import(
            &barrel,
            &primitive,
            "PrimitiveX",
            "PrimitiveX",
        )];
        let exports = vec![
            named_export(&primitive, "PrimitiveX"),
            reexport(&barrel, "PrimitiveX", &primitive),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        // primitive is shielded by the per-edge touch (acknowledged
        // false-negative); but the previous lenient `reexport_sources`
        // shortcut also shielded the case where the barrel was missing
        // the per-name edge. See the OUT OF SCOPE note in the module.
        assert_eq!(
            issues.len(),
            0,
            "primitive.ts:PrimitiveX is touched by the re-export edge; got: {:?}",
            issues
        );
    }

    #[test]
    fn export_default_from_named_reexport() {
        // index.ts: `export { default } from './dialog'` plus a
        // consumer doing `import D from './index'`. The engine records
        // the re-export as Named with source_name="default"; the
        // compute_orphans loop maps that into default_touched so
        // dialog.tsx's default export reads as consumed.
        let dialog = PathBuf::from("/p/dialog.tsx");
        let index = PathBuf::from("/p/index.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            named_import(&index, &dialog, "default", "default"),
            default_import(&app, &index, "D"),
        ];
        let exports = vec![
            default_export(&dialog),
            reexport(&index, "default", &dialog),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        assert!(
            issues.is_empty(),
            "`export {{ default }} from` should reach the source default; got: {:?}",
            issues
        );
    }

    #[test]
    fn star_reexport_marks_namespace_consumed() {
        // `export * from './m'` records a Namespace ImportRecord, so
        // every named export of m reads as consumed.
        let m = PathBuf::from("/p/m.ts");
        let barrel = PathBuf::from("/p/barrel.ts");
        let app = PathBuf::from("/p/app.ts");

        let imports = vec![
            namespace_import(&barrel, &m),
            named_import(&app, &barrel, "X", "X"),
        ];
        let exports = vec![
            named_export(&m, "X"),
            named_export(&m, "Y"),
            reexport(&barrel, "*", &m),
        ];
        let issues = orphan_export::compute_orphans(
            &imports,
            &exports,
            &opts_default(),
            &PublicApi::default(),
        );
        assert!(
            issues.is_empty(),
            "namespace re-export should consume all named exports; got: {:?}",
            issues
        );
    }

    #[test]
    fn unused_named_export_still_flagged() {
        // Sanity check: when nothing imports a named export, it's
        // still flagged. Guards against the fix accidentally turning
        // the check into a no-op.
        let m = PathBuf::from("/p/m.ts");
        let exports = vec![named_export(&m, "Forgotten")];
        let issues =
            orphan_export::compute_orphans(&[], &exports, &opts_default(), &PublicApi::default());
        assert_eq!(issues.len(), 1, "expected one orphan, got: {:?}", issues);
        assert!(issues[0].message.contains("Forgotten"));
    }
}
