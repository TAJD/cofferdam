//! Design checks — boundary, coupling, orphan-export. Most live in
//! `Check::finalize()` because they need the whole project graph.

use std::path::Path;

mod barrel_reexport_bloat;
mod boundary_frozen;
mod class_as_data_bag;
mod duplicate_export_name;
mod duplicate_type_shape;
mod effect_leakage;
mod import_cycle;
mod import_fan_out_outlier;
mod invariant_violation;
mod layer_violation;
mod max_parameters;
mod missing_test_file;
mod orphan_export;
mod package_entry_point;
mod readonly_array_param;
mod scripted_invariant;
mod union_exhaustiveness_gap;

pub use barrel_reexport_bloat::BarrelReexportBloat;
pub use boundary_frozen::BoundaryFrozen;
pub use class_as_data_bag::ClassAsDataBag;
pub use duplicate_export_name::DuplicateExportName;
pub use duplicate_type_shape::DuplicateTypeShape;
pub use effect_leakage::EffectLeakage;
pub use import_cycle::ImportCycle;
pub use import_fan_out_outlier::ImportFanOutOutlier;
pub use invariant_violation::InvariantViolation;
pub use layer_violation::LayerViolation;
pub use max_parameters::{max_in_file as max_parameters_in_file, MaxParameters};
pub use missing_test_file::MissingTestFile;
pub use orphan_export::OrphanExport;
pub use readonly_array_param::ReadonlyArrayParam;
pub use scripted_invariant::ScriptedInvariant;
pub use union_exhaustiveness_gap::UnionExhaustivenessGap;

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
    fn package_json_entry_with_no_manifest_matches_nothing_and_says_so() {
        let root = PathBuf::from("/project");
        let api = make_public_api(&["package.json:exports"], &root);
        let key = path_key(&root.join("src/index.ts"));
        assert!(!api.is_match(&key));
        assert_eq!(
            api.unresolved(),
            ["package.json:exports".to_string()],
            "a pointer at a manifest that isn't there must be reported, not dropped"
        );
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
            test_imports_count: true,
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

    // ── Design.UnionExhaustivenessGap (CD-118) ─────────────────────────
    //
    // The decision logic is type-driven, so a stub oracle stands in for
    // the ts-morph host: it returns the same UnionFacts for every query,
    // which is enough because each test source has one switch statement.
    // The real worker path (`unionMembers` RPC) is a gated end-to-end
    // test in cofferdam-cli, same pattern as `worker_oracle_resolves_*`
    // in type_host.rs.

    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{
        Allocator, Check, CheckContext, Issue as CoreIssue, SourceFile, TypeOracle, UnionFacts,
    };
    use union_exhaustiveness_gap::UnionExhaustivenessGap;

    struct FixedUnionOracle(Option<UnionFacts>);
    impl TypeOracle for FixedUnionOracle {
        fn type_at(&self, _f: &Path, _s: u32, _e: u32) -> Option<cofferdam_core::TypeFacts> {
            None
        }
        fn union_members_at(&self, _f: &Path, _s: u32, _e: u32) -> Option<UnionFacts> {
            self.0.clone()
        }
    }

    fn union_facts(members: &[&str]) -> UnionFacts {
        UnionFacts {
            members: members.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn run_union_exhaustiveness(src: &str, oracle_result: Option<UnionFacts>) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let oracle = FixedUnionOracle(oracle_result);
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&parsed)
            .with_types(&oracle);
        UnionExhaustivenessGap.run(&file, &mut ctx)
    }

    #[test]
    fn missing_variant_no_default_is_flagged() {
        let src = r#"
            function area(shape: Shape) {
              switch (shape.kind) {
                case "circle": return 1;
                case "square": return 2;
              }
            }
        "#;
        let issues =
            run_union_exhaustiveness(src, Some(union_facts(&["circle", "square", "triangle"])));
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("triangle"));
    }

    #[test]
    fn all_variants_handled_is_not_flagged() {
        let src = r#"
            function area(shape: Shape) {
              switch (shape.kind) {
                case "circle": return 1;
                case "square": return 2;
                case "triangle": return 3;
              }
            }
        "#;
        let issues =
            run_union_exhaustiveness(src, Some(union_facts(&["circle", "square", "triangle"])));
        assert!(issues.is_empty(), "expected no findings; got {issues:?}");
    }

    #[test]
    fn default_case_suppresses_finding_even_with_missing_variant() {
        let src = r#"
            function area(shape: Shape) {
              switch (shape.kind) {
                case "circle": return 1;
                default: return 0;
              }
            }
        "#;
        let issues =
            run_union_exhaustiveness(src, Some(union_facts(&["circle", "square", "triangle"])));
        assert!(
            issues.is_empty(),
            "default case should suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn numeric_discriminant_missing_variant_is_flagged() {
        let src = r#"
            function area(shape: Shape) {
              switch (shape.kind) {
                case 0: return 1;
                case 1: return 2;
              }
            }
        "#;
        let issues = run_union_exhaustiveness(src, Some(union_facts(&["0", "1", "2"])));
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains('2'));
    }

    #[test]
    fn boolean_discriminant_all_variants_handled_is_not_flagged() {
        let src = r#"
            function f(shape: Shape) {
              switch (shape.flag) {
                case true: return 1;
                case false: return 2;
              }
            }
        "#;
        let issues = run_union_exhaustiveness(src, Some(union_facts(&["true", "false"])));
        assert!(issues.is_empty(), "expected no findings; got {issues:?}");
    }

    #[test]
    fn non_literal_union_discriminant_is_not_flagged() {
        // Oracle returns None — the discriminant isn't a literal-only
        // union (e.g. a plain `string`, or a union with a non-literal
        // member) — the check must bail entirely.
        let src = r#"
            function f(x: string) {
              switch (x) {
                case "a": return 1;
              }
            }
        "#;
        let issues = run_union_exhaustiveness(src, None);
        assert!(
            issues.is_empty(),
            "non-literal-union discriminant must not flag; got {issues:?}"
        );
    }

    // ─── Design.ClassAsDataBag (CD-121) ─────────────────────────────────

    use class_as_data_bag::ClassAsDataBag;

    fn run_class_as_data_bag(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        ClassAsDataBag.run(&file, &mut ctx)
    }

    #[test]
    fn field_only_constructor_is_flagged() {
        let src = "\
class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}";
        let issues = run_class_as_data_bag(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("Point"));
    }

    #[test]
    fn no_constructor_plain_fields_is_flagged() {
        let src = "\
class Bare {
  a: number = 0;
  b: string = \"\";
}";
        let issues = run_class_as_data_bag(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn parameter_properties_are_not_flagged() {
        let src = "\
class ConfigService {
  constructor(private readonly apiUrl: string) {}
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "parameter properties must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn implements_clause_is_not_flagged() {
        let src = "\
interface Shape { area(): number; }
class Square implements Shape {
  constructor(public side: number) {}
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "implements clause must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn extends_builtin_is_not_flagged() {
        let src = "\
class NotFoundError extends Error {
  constructor(public id: string) {
    super(`not found: ${id}`);
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a superclass (even a built-in) must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn decorated_class_is_not_flagged() {
        let src = "\
@Injectable()
class Config {
  constructor(public apiUrl: string) {}
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a class decorator must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn constructor_with_validation_is_not_flagged() {
        let src = "\
class Validated {
  x: number;
  constructor(x: number) {
    if (x < 0) {
      throw new Error(\"x must be non-negative\");
    }
    this.x = x;
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "constructor logic beyond field assignment must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn class_with_real_method_is_not_flagged() {
        let src = "\
class Counter {
  count: number;
  constructor() {
    this.count = 0;
  }
  increment(): void {
    this.count += 1;
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a real method beyond the constructor must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn compound_assignment_in_constructor_is_not_flagged() {
        // `+=` reads before writing — behavior, not initialization — even
        // though the target still looks like `this.<field>`.
        let src = "\
class Weird {
  x: number;
  constructor(x: number) {
    this.x = 0;
    this.x += x;
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a compound-assignment statement must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn bare_readonly_parameter_is_not_flagged() {
        // `readonly` alone (no accessibility keyword) is still a
        // parameter property.
        let src = "\
class Wrapper {
  constructor(readonly x: number) {}
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a bare `readonly` parameter must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn static_only_class_is_not_flagged() {
        let src = "\
class Utils {
  static VERSION: string = \"1.0\";
  static parse(s: string): number {
    return Number(s);
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a static-only utility class must not flag; got {issues:?}"
        );
    }

    #[test]
    fn getter_setter_class_is_not_flagged() {
        let src = "\
class Wrapper {
  private _x: number;
  constructor(x: number) {
    this._x = x;
  }
  get x(): number {
    return this._x;
  }
}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a getter/setter means the class has real behavior; got {issues:?}"
        );
    }

    #[test]
    fn empty_class_with_no_fields_is_not_flagged() {
        let src = "class Marker {}";
        let issues = run_class_as_data_bag(src);
        assert!(
            issues.is_empty(),
            "a genuinely empty class must not flag (marker-type pattern); got {issues:?}"
        );
    }

    #[test]
    fn skipped_entirely_without_an_oracle() {
        let file = SourceFile::new(
            PathBuf::from("test.ts"),
            "function f(shape: Shape) { switch (shape.kind) { case \"circle\": return 1; } }"
                .to_string(),
        );
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        let issues = UnionExhaustivenessGap.run(&file, &mut ctx);
        assert!(issues.is_empty(), "no oracle → no findings; got {issues:?}");
    }

    // ─── Design.ReadonlyArrayParam (CD-126) ─────────────────────────────

    use readonly_array_param::ReadonlyArrayParam;

    fn run_readonly_array_param(src: &str) -> Vec<CoreIssue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src.to_string());
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        ReadonlyArrayParam.run(&file, &mut ctx)
    }

    #[test]
    fn unmutated_array_param_is_flagged() {
        let src = "\
function total(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}";
        let issues = run_readonly_array_param(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("items"));
    }

    #[test]
    fn unmutated_object_literal_param_is_flagged() {
        let src = "\
function describe(point: { x: number; y: number }): string {
  return `(${point.x}, ${point.y})`;
}";
        let issues = run_readonly_array_param(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn array_generic_form_is_flagged() {
        let src = "\
function first(items: Array<string>): string | undefined {
  return items[0];
}";
        let issues = run_readonly_array_param(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn already_readonly_is_not_flagged() {
        let src = "\
function sum(items: readonly number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "already-readonly param must not flag; got {issues:?}"
        );
    }

    #[test]
    fn mutated_via_array_method_is_not_flagged() {
        let src = "\
function sortInPlace(items: number[]): void {
  items.sort();
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a param mutated via an array method must not flag; got {issues:?}"
        );
    }

    #[test]
    fn mutated_via_index_assignment_is_not_flagged() {
        let src = "\
function zeroOut(items: number[]): void {
  items[0] = 0;
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a param mutated via index assignment must not flag; got {issues:?}"
        );
    }

    #[test]
    fn mutated_via_field_write_is_not_flagged() {
        let src = "\
function reset(point: { x: number; y: number }): void {
  point.x = 0;
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a param mutated via a field write must not flag; got {issues:?}"
        );
    }

    #[test]
    fn passed_through_to_another_call_is_not_flagged() {
        let src = "\
function process(items: number[]): void {
  mutateSomewhere(items);
}
declare function mutateSomewhere(items: number[]): void;";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a param passed to another call must not flag (can't rule out mutation there); got {issues:?}"
        );
    }

    #[test]
    fn passed_through_to_a_constructor_is_not_flagged() {
        let src = "\
function process(items: number[]): void {
  new Wrapper(items);
}
declare class Wrapper { constructor(items: number[]); }";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a param passed to a constructor must not flag (can't rule out mutation there); got {issues:?}"
        );
    }

    #[test]
    fn readonly_array_generic_form_is_not_flagged() {
        let src = "\
function sum(items: ReadonlyArray<number>): number {
  return items.reduce((sum, n) => sum + n, 0);
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "ReadonlyArray<T> must not flag; got {issues:?}"
        );
    }

    #[test]
    fn untyped_param_is_not_flagged() {
        let src = "\
function total(items) {
  return items.length;
}";
        let issues = run_readonly_array_param(src);
        assert!(
            issues.is_empty(),
            "a parameter with no type annotation must not flag; got {issues:?}"
        );
    }

    #[test]
    fn arrow_function_param_is_flagged() {
        let src = "export const total = (items: number[]): number => items.length;";
        let issues = run_readonly_array_param(src);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    // ─── Design.EffectLeakage (CD-127) ──────────────────────────────────

    use cofferdam_core::graph::EXPORTS as GRAPH_EXPORTS;
    use cofferdam_core::graph::IMPORTS as GRAPH_IMPORTS;
    use cofferdam_core::{CorpusIndex, FinalizeContext};
    use effect_leakage::EffectLeakage;

    fn external_import(from: &Path, specifier: &str) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: specifier.to_string(),
            resolved: None,
            names: Vec::new(),
            type_only: false,
            span: span(),
        }
    }

    fn internal_import(from: &Path, resolved: &Path) -> ImportRecord {
        named_import(from, resolved, "x", "x")
    }

    /// Runs `EffectLeakage.run()` over each `(path, source)` fixture,
    /// seeds the corpus's shared import graph with `extra_imports`
    /// (standing in for the engine's own graph-builder pass, which
    /// doesn't run in this unit test), then calls `.finalize()`.
    fn run_effect_leakage(
        fixtures: &[(&Path, &str)],
        extra_imports: Vec<ImportRecord>,
    ) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::new();
        for (path, src) in fixtures {
            let file = SourceFile::new(path.to_path_buf(), src.to_string());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            EffectLeakage.run(&file, &mut ctx);
        }
        corpus.with_slot(&GRAPH_IMPORTS, |slot| {
            slot.extend_by(extra_imports, |r| &r.from_file)
        });
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        EffectLeakage.finalize(&mut finalize_ctx)
    }

    #[test]
    fn direct_import_of_side_effecting_module_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(&a, "// @pure\nexport function f() {}")],
            vec![external_import(&a, "fs")],
        );
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("fs"));
    }

    #[test]
    fn transitive_import_of_side_effecting_module_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_effect_leakage(
            &[
                (&a, "// @pure\nexport function f() {}"),
                (&b, "export function g() {}"),
            ],
            vec![internal_import(&a, &b), external_import(&b, "fs")],
        );
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].file == a);
    }

    #[test]
    fn genuinely_pure_chain_is_not_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_effect_leakage(
            &[
                (&a, "// @pure\nexport function f() {}"),
                (&b, "export function g() {}"),
            ],
            vec![internal_import(&a, &b)],
        );
        assert!(
            issues.is_empty(),
            "a chain with no side-effecting module must not flag; got {issues:?}"
        );
    }

    #[test]
    fn no_pure_tag_is_not_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(&a, "export function f() {}")],
            vec![external_import(&a, "fs")],
        );
        assert!(
            issues.is_empty(),
            "a file with no @pure tag must not flag, even with a direct fs import; got {issues:?}"
        );
    }

    #[test]
    fn prose_mentioning_pure_is_not_mistaken_for_the_tag() {
        // A comment that merely mentions "@pure" in prose (not as its own
        // JSDoc-style tag line) must not opt the file in.
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(
                &a,
                "// TODO: consider marking this @pure eventually\nexport function f() {}",
            )],
            vec![external_import(&a, "fs")],
        );
        assert!(
            issues.is_empty(),
            "prose mentioning @pure must not opt the file in; got {issues:?}"
        );
    }

    #[test]
    fn node_prefixed_specifier_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(&a, "// @pure\nexport function f() {}")],
            vec![external_import(&a, "node:fs")],
        );
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn non_denylisted_external_module_is_not_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(&a, "// @pure\nexport function f() {}")],
            vec![external_import(&a, "lodash")],
        );
        assert!(
            issues.is_empty(),
            "an external module not on the denylist must not flag; got {issues:?}"
        );
    }

    #[test]
    fn import_cycle_does_not_hang_the_bfs() {
        // a -> b -> a, with b also reaching fs. The cycle must not cause
        // an infinite loop, and the reachable side effect must still be
        // found.
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_effect_leakage(
            &[
                (&a, "// @pure\nexport function f() {}"),
                (&b, "export function g() {}"),
            ],
            vec![
                internal_import(&a, &b),
                internal_import(&b, &a),
                external_import(&b, "fs"),
            ],
        );
        assert_eq!(
            issues.len(),
            1,
            "an import cycle must not hang the BFS and the reachable side effect must still be found; got {issues:?}"
        );
    }

    #[test]
    fn block_comment_pure_tag_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(
                &a,
                "/**\n * A helper.\n * @pure\n * @param x\n */\nexport function f() {}",
            )],
            vec![external_import(&a, "fs")],
        );
        assert_eq!(
            issues.len(),
            1,
            "a @pure tag inside a multi-line JSDoc block comment must be recognized; got {issues:?}"
        );
    }

    // ─── Design.EffectLeakage (CD-138): per-function granularity ────────

    fn internal_import_with_specifier(
        from: &Path,
        resolved: &Path,
        specifier: &str,
    ) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: specifier.to_string(),
            resolved: Some(resolved.to_path_buf()),
            names: Vec::new(),
            type_only: false,
            span: span(),
        }
    }

    #[test]
    fn pure_tag_on_a_specific_function_does_not_flag_an_unrelated_functions_import() {
        // The @pure tag is attached to `computeTotal`, not `loadConfig` —
        // `fs` is only referenced inside `loadConfig`'s own body, so it
        // must not leak into `computeTotal`'s purity check.
        let a = PathBuf::from("/p/a.ts");
        let source = r#"
import * as fs from "fs";

export function loadConfig(): string {
  return fs.readFileSync("config.json", "utf8");
}

// @pure
export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0);
}
"#;
        let issues = run_effect_leakage(&[(&a, source)], vec![external_import(&a, "fs")]);
        assert!(
            issues.is_empty(),
            "an fs import used only by an unrelated function in the same file must not flag the \
             @pure-tagged function; got {issues:?}"
        );
    }

    #[test]
    fn pure_tag_on_a_specific_function_flags_when_its_own_body_uses_the_side_effect() {
        let a = PathBuf::from("/p/a.ts");
        let source = r#"
import * as fs from "fs";

export function unrelated(): number {
  return 1;
}

// @pure
export function loadConfig(): string {
  return fs.readFileSync("config.json", "utf8");
}
"#;
        let issues = run_effect_leakage(&[(&a, source)], vec![external_import(&a, "fs")]);
        assert_eq!(
            issues.len(),
            1,
            "the @pure-tagged function's own direct use of fs must be flagged; got {issues:?}"
        );
    }

    #[test]
    fn pure_tag_on_a_specific_function_flags_a_transitive_side_effect_through_its_own_import() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let source_a = r#"
import { readFromDisk } from "./b";

export function unrelated(): number {
  return 1;
}

// @pure
export function computeTotal(items: number[]): number {
  return items.reduce((sum, n) => sum + n, 0) + readFromDisk().length;
}
"#;
        let issues = run_effect_leakage(
            &[
                (&a, source_a),
                (
                    &b,
                    "export function readFromDisk(): string { return \"\"; }",
                ),
            ],
            vec![
                internal_import_with_specifier(&a, &b, "./b"),
                external_import(&b, "fs"),
            ],
        );
        assert_eq!(
            issues.len(),
            1,
            "a side effect reached one hop through the @pure-tagged function's own import must \
             still be flagged; got {issues:?}"
        );
    }

    #[test]
    fn subpath_of_a_denylisted_module_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_effect_leakage(
            &[(&a, "// @pure\nexport function f() {}")],
            vec![external_import(&a, "mysql2/promise")],
        );
        assert_eq!(
            issues.len(),
            1,
            "a subpath of a denylisted module (mysql2/promise) must be flagged; got {issues:?}"
        );
    }

    #[test]
    fn extra_side_effect_modules_extends_the_denylist() {
        use effect_leakage::side_effecting_match;
        assert_eq!(
            side_effecting_match(
                "some-custom-db-client",
                &["some-custom-db-client".to_string()]
            ),
            Some("some-custom-db-client".to_string()),
            "a module named via extra_side_effect_modules must be recognized"
        );
        assert_eq!(
            side_effecting_match("some-custom-db-client", &[]),
            None,
            "a module not on either list must not be recognized"
        );
    }

    // ─── Design.DuplicateTypeShape (CD-128) ─────────────────────────────

    use duplicate_type_shape::DuplicateTypeShape;

    /// Runs `DuplicateTypeShape.run()` over each `(path, source)` fixture,
    /// then `.finalize()`, mirroring `run_effect_leakage`'s single-corpus
    /// harness.
    fn run_duplicate_type_shape(fixtures: &[(&Path, &str)]) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::new();
        for (path, src) in fixtures {
            let file = SourceFile::new(path.to_path_buf(), src.to_string());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            DuplicateTypeShape.run(&file, &mut ctx);
        }
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        DuplicateTypeShape.finalize(&mut finalize_ctx)
    }

    #[test]
    fn identical_interfaces_across_files_are_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "export interface User { id: string; name: string; email: string; }",
            ),
            (
                &b,
                "export interface Customer { id: string; name: string; email: string; }",
            ),
        ]);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert!(issues[0].message.contains("User"));
        assert!(issues[0].message.contains("Customer"));
    }

    #[test]
    fn identical_type_literal_aliases_are_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "export type A = { id: string; name: string; email: string; };",
            ),
            (
                &b,
                "export type B = { id: string; name: string; email: string; };",
            ),
        ]);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn below_min_fields_is_not_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (&a, "export interface Point { x: number; y: number; }"),
            (&b, "export interface Size { x: number; y: number; }"),
        ]);
        assert!(
            issues.is_empty(),
            "a 2-field shape must not be flagged even if identical; got {issues:?}"
        );
    }

    #[test]
    fn divergent_shapes_are_not_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "export interface Draft { id: string; title: string; body: string; }",
            ),
            (
                &b,
                "export interface Published { id: string; title: string; publishedAt: string; }",
            ),
        ]);
        assert!(
            issues.is_empty(),
            "shapes below the similarity threshold must not be flagged; got {issues:?}"
        );
    }

    #[test]
    fn interface_with_extends_is_not_compared_against_a_non_extending_peer() {
        // An interface with a non-empty `extends` is only ever comparable
        // to another interface sharing the exact same base set (CD-135)
        // — it must not match a peer with no `extends` at all, even if
        // its own declared body happens to line up.
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "interface Base { id: string; } \
                 export interface User extends Base { name: string; email: string; }",
            ),
            (
                &b,
                "export interface Customer { id: string; name: string; email: string; }",
            ),
        ]);
        assert!(
            issues.is_empty(),
            "an interface with extends must not match a non-extending peer; got {issues:?}"
        );
    }

    #[test]
    fn single_file_pair_is_flagged() {
        let a = PathBuf::from("/p/a.ts");
        let issues = run_duplicate_type_shape(&[(
            &a,
            "export interface User { id: string; name: string; email: string; } \
             export interface Customer { id: string; name: string; email: string; }",
        )]);
        assert_eq!(
            issues.len(),
            1,
            "a duplicate pair within a single file must still be flagged; got {issues:?}"
        );
    }

    #[test]
    fn extends_clause_interfaces_sharing_a_base_are_flagged() {
        // Both interfaces extend the same base and add no fields of
        // their own — before CD-135 this was invisible (interfaces with
        // extends were skipped entirely); the shared base alone is now a
        // full duplication signal even with zero own fields.
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "interface Base { id: string; name: string; email: string; } \
                 export interface User extends Base {}",
            ),
            (&b, "export interface Customer extends Base {}"),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "two interfaces extending the same base with no own fields must be flagged; got {issues:?}"
        );
        assert!(issues[0].message.contains("User"));
        assert!(issues[0].message.contains("Customer"));
    }

    #[test]
    fn extends_clause_interfaces_with_different_bases_are_not_flagged() {
        // Different bases mean the effective (inherited) shape differs
        // even though the declared bodies happen to match — must not be
        // flagged just because both declare the same two own fields.
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "interface BaseA { id: string; } \
                 export interface User extends BaseA { name: string; email: string; }",
            ),
            (
                &b,
                "interface BaseB { id: string; } \
                 export interface Customer extends BaseB { name: string; email: string; }",
            ),
        ]);
        assert!(
            issues.is_empty(),
            "interfaces extending different bases must not be flagged even with matching own fields; got {issues:?}"
        );
    }

    #[test]
    fn three_mutually_similar_shapes_are_clustered_into_one_finding() {
        // Three independently declared interfaces with the same shape
        // must produce ONE finding grouping all three, not three
        // separate pairwise findings (CD-135).
        let a = PathBuf::from("/p/a.ts");
        let b = PathBuf::from("/p/b.ts");
        let c = PathBuf::from("/p/c.ts");
        let issues = run_duplicate_type_shape(&[
            (
                &a,
                "export interface User { id: string; name: string; email: string; }",
            ),
            (
                &b,
                "export interface Customer { id: string; name: string; email: string; }",
            ),
            (
                &c,
                "export interface Contact { id: string; name: string; email: string; }",
            ),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "three mutually-similar shapes must be clustered into one finding; got {issues:?}"
        );
        assert_eq!(
            issues[0].related.len(),
            2,
            "the one finding must reference both other members of the cluster; got {issues:?}"
        );
        assert!(issues[0].message.contains("User"));
        assert!(issues[0].message.contains("Customer"));
        assert!(issues[0].message.contains("Contact"));
    }

    // ─── Design.ImportFanOutOutlier (CD-130) ────────────────────────────

    use import_fan_out_outlier::ImportFanOutOutlier;

    /// Runs `ImportFanOutOutlier.finalize()` over a corpus seeded with
    /// `imports` as the shared import graph.
    fn run_fan_out_outlier(imports: Vec<ImportRecord>) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::new();
        corpus.with_slot(&GRAPH_IMPORTS, |slot| {
            slot.extend_by(imports, |r| &r.from_file)
        });
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        ImportFanOutOutlier.finalize(&mut finalize_ctx)
    }

    /// 14 zero-fan-out/zero-fan-in leaves plus one file that imports all
    /// of them — comfortably past the 3-sigma threshold (verified by
    /// hand: for n=15 total with 14 zeros and one outlier v, the ratio
    /// mean/stddev shrinks the threshold below v for n >= 11).
    fn god_and_leaves_imports(god: &Path, leaf_prefix: &str, count: usize) -> Vec<ImportRecord> {
        (0..count)
            .map(|i| {
                let leaf = PathBuf::from(format!("/p/{leaf_prefix}{i}.ts"));
                internal_import(god, &leaf)
            })
            .collect()
    }

    #[test]
    fn fan_out_outlier_is_flagged() {
        let god = PathBuf::from("/p/god.ts");
        let imports = god_and_leaves_imports(&god, "leaf", 14);
        let issues = run_fan_out_outlier(imports);
        assert_eq!(
            issues.len(),
            1,
            "expected one fan-out finding for the god file; got {issues:?}"
        );
        assert_eq!(issues[0].file, god);
        assert!(issues[0].message.contains("fan-out"));
    }

    #[test]
    fn high_fan_in_alone_is_not_flagged() {
        // CD-333: 14 files that each import a single shared `utils.ts`
        // — utils has fan-in 14 against a background of zero, but zero
        // fan-out of its own. A shared leaf module doing its job must
        // not be flagged; fan-in alone can't distinguish it from a
        // real hub.
        let utils = PathBuf::from("/p/utils.ts");
        let imports: Vec<ImportRecord> = (0..14)
            .map(|i| {
                let importer = PathBuf::from(format!("/p/importer{i}.ts"));
                internal_import(&importer, &utils)
            })
            .collect();
        let issues = run_fan_out_outlier(imports);
        assert!(
            issues.is_empty(),
            "high fan-in alone must not be flagged; got {issues:?}"
        );
    }

    #[test]
    fn high_fan_in_and_fan_out_together_is_flagged() {
        // CD-333: a genuine god module — high fan-in AND high fan-out
        // — must still fire the fan-in finding. 14 sibling files each
        // import `hub.ts` (hub's fan-in) and `hub.ts` imports each of
        // them back (hub's fan-out).
        let hub = PathBuf::from("/p/hub.ts");
        let mut imports = Vec::new();
        for i in 0..14 {
            let sibling = PathBuf::from(format!("/p/sibling{i}.ts"));
            imports.push(internal_import(&sibling, &hub));
            imports.push(internal_import(&hub, &sibling));
        }
        let issues = run_fan_out_outlier(imports);
        let fan_in_issues: Vec<&CoreIssue> = issues
            .iter()
            .filter(|i| i.message.contains("fan-in"))
            .collect();
        assert_eq!(
            fan_in_issues.len(),
            1,
            "expected one fan-in finding for the god module; got {issues:?}"
        );
        assert_eq!(fan_in_issues[0].file, hub);
    }

    #[test]
    fn below_min_files_emits_nothing() {
        // Only 3 files total, well under MIN_FILES (8).
        let god = PathBuf::from("/p/god.ts");
        let imports = god_and_leaves_imports(&god, "leaf", 2);
        let issues = run_fan_out_outlier(imports);
        assert!(
            issues.is_empty(),
            "too few files must not flag anything; got {issues:?}"
        );
    }

    #[test]
    fn hub_index_file_is_excluded_from_population_and_flagging() {
        // Same shape as fan_out_outlier_is_flagged, but the "god" file is
        // named index.ts — it must be excluded entirely, not just from
        // flagging, so the leaves' fan-in (from index.ts) shouldn't
        // count either.
        let god = PathBuf::from("/p/index.ts");
        let imports = god_and_leaves_imports(&god, "leaf", 14);
        let issues = run_fan_out_outlier(imports);
        assert!(
            issues.is_empty(),
            "an index.ts hub must be excluded entirely; got {issues:?}"
        );
    }

    #[test]
    fn uniform_counts_emit_nothing() {
        // Every file has the same fan-out (1) — stddev is 0, so nothing
        // can be an outlier.
        let imports: Vec<ImportRecord> = (0..8)
            .map(|i| {
                let from = PathBuf::from(format!("/p/f{i}.ts"));
                let to = PathBuf::from(format!("/p/f{}.ts", (i + 1) % 8));
                internal_import(&from, &to)
            })
            .collect();
        let issues = run_fan_out_outlier(imports);
        assert!(
            issues.is_empty(),
            "uniform fan-out/fan-in (stddev 0) must not flag anything; got {issues:?}"
        );
    }

    #[test]
    fn node_modules_resolved_target_is_excluded() {
        // 14 in-project files all import a bare specifier that the
        // resolver traced into node_modules — same shape as
        // `fan_in_outlier_is_flagged` (n=15, one value of 14 rest 0),
        // which *would* flag if the vendor file weren't excluded. Uses a
        // non-hub basename so `is_hub_file` can't accidentally account
        // for the exclusion instead of the node_modules check.
        let vendor = PathBuf::from("/p/node_modules/leftpad/leftpad.js");
        let imports: Vec<ImportRecord> = (0..14)
            .map(|i| {
                let importer = PathBuf::from(format!("/p/importer{i}.ts"));
                internal_import(&importer, &vendor)
            })
            .collect();
        let issues = run_fan_out_outlier(imports);
        assert!(
            issues.is_empty(),
            "a node_modules-resolved target must not be flagged, nor skew the population; got {issues:?}"
        );
    }

    #[test]
    fn differently_named_entry_point_is_excluded_via_package_json() {
        // Same god+14-leaves shape as `fan_out_outlier_is_flagged` (which
        // *is* flagged), but the "god" file is named `container.ts` — not
        // in HUB_BASENAMES — and resolves as the nearest package.json's
        // `main` entry point (CD-133). Must be excluded entirely, the
        // same as an index.ts hub.
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!(
            "cofferdam-test-fanout-entry-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut pkg = std::fs::File::create(tmp.join("package.json")).unwrap();
        write!(pkg, r#"{{"main": "./container.ts"}}"#).unwrap();
        drop(pkg);

        let container = tmp.join("container.ts");
        let imports = god_and_leaves_imports(&container, "leaf", 14);
        let issues = run_fan_out_outlier(imports);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            issues.is_empty(),
            "a differently-named file resolved as package.json's main entry point must be \
             excluded entirely, same as an index.ts hub; got {issues:?}"
        );
    }

    #[test]
    fn isolated_file_with_no_edges_still_enters_population() {
        // Same god+14-leaves shape as `fan_out_outlier_is_flagged` (n=15,
        // mean 14/15 ~= 0.9), plus 5 additional files with neither an
        // import nor an export of their own — invisible to
        // GRAPH_IMPORTS/GRAPH_EXPORTS, so only run()'s ALL_FILES corpus
        // write (CD-133) can surface them. If they enter the population,
        // the reported mean must shift to reflect n=20 (14/20 = 0.7)
        // rather than n=15.
        let god = PathBuf::from("/p/god.ts");
        let imports = god_and_leaves_imports(&god, "leaf", 14);

        let corpus = CorpusIndex::new();
        corpus.with_slot(&GRAPH_IMPORTS, |slot| {
            slot.extend_by(imports, |r| &r.from_file)
        });
        for i in 0..5 {
            let path = PathBuf::from(format!("/p/isolated{i}.ts"));
            let file = SourceFile::new(path, String::new());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            ImportFanOutOutlier.run(&file, &mut ctx);
        }
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        let issues = ImportFanOutOutlier.finalize(&mut finalize_ctx);

        assert_eq!(
            issues.len(),
            1,
            "god file must still be flagged for fan-out; got {issues:?}"
        );
        assert!(
            issues[0].message.contains("mean 0.7"),
            "population must include the 5 isolated files (n=20, mean 14/20=0.7), not just the \
             15 graph-derived files (n=15, mean 14/15=0.9); got: {}",
            issues[0].message
        );
    }

    // ─── Design.BarrelReexportBloat (CD-131) ────────────────────────────

    use barrel_reexport_bloat::BarrelReexportBloat;

    fn run_barrel_reexport_bloat(exports: Vec<ExportRecord>) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::new();
        corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.extend_by(exports, |r| &r.file));
        let mut finalize_ctx = FinalizeContext::new(&corpus);
        BarrelReexportBloat.finalize(&mut finalize_ctx)
    }

    fn barrel_reexport(file: &Path, name: &str) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::ReExport,
            type_only: false,
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
            source_specifier: None,
            resolved_source: None,
        }
    }

    fn barrel_real_export(file: &Path, name: &str) -> ExportRecord {
        ExportRecord {
            file: file.to_path_buf(),
            name: name.to_string(),
            kind: ExportKind::Named,
            type_only: false,
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
            source_specifier: None,
            resolved_source: None,
        }
    }

    /// `sparse_count` barrels, each re-exporting 1 of `sibling_real`
    /// sibling exports, plus one bloated barrel re-exporting
    /// `bloated_ratio_num` of the same `sibling_real` siblings, each
    /// barrel in its own directory. Mirrors
    /// `ImportFanOutOutlier`'s hand-verified n>=11 3-sigma shape: with
    /// (n-1) near-zero background ratios and one outlier v, flagging
    /// requires `1 > 1/n + 3/sqrt(n)`, so `sparse_count` must be at
    /// least 10 for the outlier to actually breach the threshold.
    fn barrel_fixture(
        bloated_ratio_num: u32,
        sibling_real: u32,
        sparse_count: usize,
    ) -> Vec<ExportRecord> {
        let mut exports = Vec::new();
        for i in 0..sparse_count {
            let dir = PathBuf::from(format!("/p/mod{i}"));
            for j in 0..sibling_real {
                exports.push(barrel_real_export(
                    &dir.join(format!("f{j}.ts")),
                    &format!("x{j}"),
                ));
            }
            exports.push(barrel_reexport(&dir.join("index.ts"), "x0"));
        }
        let bloated_dir = PathBuf::from("/p/bloated");
        for j in 0..sibling_real {
            exports.push(barrel_real_export(
                &bloated_dir.join(format!("f{j}.ts")),
                &format!("x{j}"),
            ));
        }
        for j in 0..bloated_ratio_num {
            exports.push(barrel_reexport(
                &bloated_dir.join("index.ts"),
                &format!("x{j}"),
            ));
        }
        exports
    }

    #[test]
    fn bloated_barrel_is_flagged() {
        let exports = barrel_fixture(50, 100, 14);
        let issues = run_barrel_reexport_bloat(exports);
        assert_eq!(
            issues.len(),
            1,
            "expected one bloat finding for the outlier barrel; got {issues:?}"
        );
        assert_eq!(issues[0].file, PathBuf::from("/p/bloated/index.ts"));
    }

    #[test]
    fn below_min_barrels_emits_nothing() {
        // Only 2 barrel candidates total, under MIN_BARRELS (5).
        let exports = barrel_fixture(50, 100, 1);
        let issues = run_barrel_reexport_bloat(exports);
        assert!(
            issues.is_empty(),
            "too few barrel candidates must not flag anything; got {issues:?}"
        );
    }

    #[test]
    fn uniform_ratios_emit_nothing() {
        // Every barrel has the same 1/100 ratio — stddev is 0.
        let exports = barrel_fixture(1, 100, 5);
        let issues = run_barrel_reexport_bloat(exports);
        assert!(
            issues.is_empty(),
            "uniform re-export ratios (stddev 0) must not flag anything; got {issues:?}"
        );
    }

    #[test]
    fn package_entry_point_is_excluded() {
        use std::io::Write;
        let tmp =
            std::env::temp_dir().join(format!("cofferdam-test-barrel-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("bloated")).unwrap();
        let mut pkg = std::fs::File::create(tmp.join("package.json")).unwrap();
        write!(pkg, r#"{{"main": "./bloated/index.ts"}}"#).unwrap();
        drop(pkg);

        let mut exports = Vec::new();
        for i in 0..14 {
            let dir = tmp.join(format!("mod{i}"));
            for j in 0..100u32 {
                exports.push(barrel_real_export(
                    &dir.join(format!("f{j}.ts")),
                    &format!("x{j}"),
                ));
            }
            exports.push(barrel_reexport(&dir.join("index.ts"), "x0"));
        }
        let bloated_dir = tmp.join("bloated");
        for j in 0..100u32 {
            exports.push(barrel_real_export(
                &bloated_dir.join(format!("f{j}.ts")),
                &format!("x{j}"),
            ));
        }
        for j in 0..50u32 {
            exports.push(barrel_reexport(
                &bloated_dir.join("index.ts"),
                &format!("x{j}"),
            ));
        }

        let issues = run_barrel_reexport_bloat(exports);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            issues.is_empty(),
            "a file resolved as the package's main entry point must be excluded, and the \
             remaining sparse barrels have identical ratios (stddev 0); got {issues:?}"
        );
    }

    #[test]
    fn dist_entry_point_is_excluded_via_src_sibling() {
        // package.json's main points at compiled dist/index.js; the
        // analyzed source lives in a parallel src/index.ts. Without
        // CD-134's build-dir-alias swap, src/index.ts's extension-
        // stripped path never matches "dist/index" and it would be
        // flagged instead of excluded (same sparse-background shape as
        // package_entry_point_is_excluded, just with a src/dist split).
        use std::io::Write;
        let tmp =
            std::env::temp_dir().join(format!("cofferdam-test-barrel-dist-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::create_dir_all(tmp.join("dist")).unwrap();
        let mut pkg = std::fs::File::create(tmp.join("package.json")).unwrap();
        write!(pkg, r#"{{"main": "./dist/index.js"}}"#).unwrap();
        drop(pkg);

        let mut exports = Vec::new();
        for i in 0..14 {
            let dir = tmp.join(format!("mod{i}"));
            for j in 0..100u32 {
                exports.push(barrel_real_export(
                    &dir.join(format!("f{j}.ts")),
                    &format!("x{j}"),
                ));
            }
            exports.push(barrel_reexport(&dir.join("index.ts"), "x0"));
        }
        let src_dir = tmp.join("src");
        for j in 0..100u32 {
            exports.push(barrel_real_export(
                &src_dir.join(format!("f{j}.ts")),
                &format!("x{j}"),
            ));
        }
        for j in 0..50u32 {
            exports.push(barrel_reexport(&src_dir.join("index.ts"), &format!("x{j}")));
        }

        let issues = run_barrel_reexport_bloat(exports);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            issues.is_empty(),
            "src/index.ts must be excluded as the package's main entry point via the dist->src \
             alias swap; got {issues:?}"
        );
    }

    #[test]
    fn mega_barrel_of_barrels_is_counted_via_subtree_fallback() {
        // /p/mega/index.ts re-exports from two sub-barrels
        // (/p/mega/a/index.ts, /p/mega/b/index.ts) and has no
        // same-directory real sibling of its own (its only same-
        // directory siblings are the sub-barrels). Before CD-134 this
        // shape's sibling_real was 0 and the mega barrel was skipped
        // outright; the subtree-wide fallback now counts the real
        // exports nested under it instead.
        let mut exports = Vec::new();
        for i in 0..14 {
            let dir = PathBuf::from(format!("/p/sparse{i}"));
            for j in 0..100u32 {
                exports.push(barrel_real_export(
                    &dir.join(format!("f{j}.ts")),
                    &format!("x{j}"),
                ));
            }
            exports.push(barrel_reexport(&dir.join("index.ts"), "x0"));
        }

        let mega_dir = PathBuf::from("/p/mega");
        for sub in ["a", "b"] {
            let sub_dir = mega_dir.join(sub);
            for j in 0..10u32 {
                exports.push(barrel_real_export(
                    &sub_dir.join(format!("f{j}.ts")),
                    &format!("y{j}"),
                ));
            }
            exports.push(barrel_reexport(&sub_dir.join("index.ts"), "y0"));
        }
        for j in 0..10u32 {
            exports.push(barrel_reexport(
                &mega_dir.join("index.ts"),
                &format!("mega{j}"),
            ));
        }

        let issues = run_barrel_reexport_bloat(exports);
        assert!(
            issues.iter().any(|i| i.file == mega_dir.join("index.ts")),
            "the mega barrel-of-barrels must be flagged via the subtree-wide fallback since its \
             same-directory sibling_real is 0; got {issues:?}"
        );
    }

    // ─── Design.MissingTestFile (CD-132) ────────────────────────────────

    use missing_test_file::compute_missing_test_files;

    const MTF_TEST_MATCH_PATTERNS: &[&str] = &["{name}.test.ts", "__tests__/{name}.test.ts"];
    const MTF_TEST_FILE_PATTERNS: &[&str] = &[".test.", "/__tests__/"];
    const MTF_FRAMEWORK_PATTERNS: &[&str] = &["/page."];

    fn owned(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn run_missing_test_file(
        imports: Vec<ImportRecord>,
        exports: Vec<ExportRecord>,
    ) -> Vec<CoreIssue> {
        run_missing_test_file_asserting(imports, exports, Default::default())
    }

    fn run_missing_test_file_asserting(
        imports: Vec<ImportRecord>,
        exports: Vec<ExportRecord>,
        type_asserted: std::collections::HashMap<String, std::collections::HashSet<String>>,
    ) -> Vec<CoreIssue> {
        compute_missing_test_files(
            &imports,
            &exports,
            &type_asserted,
            &owned(MTF_TEST_MATCH_PATTERNS),
            &owned(MTF_TEST_FILE_PATTERNS),
            &owned(MTF_FRAMEWORK_PATTERNS),
        )
    }

    #[test]
    fn real_export_with_no_test_file_is_flagged() {
        let file = PathBuf::from("/p/format.ts");
        let exports = vec![barrel_real_export(&file, "formatCurrency")];
        let issues = run_missing_test_file(vec![], exports);
        assert_eq!(
            issues.len(),
            1,
            "expected one finding for a file with no test file; got {issues:?}"
        );
        assert_eq!(issues[0].file, file);
    }

    #[test]
    fn same_directory_test_file_suppresses_finding() {
        let file = PathBuf::from("/p/format.ts");
        let test_file = PathBuf::from("/p/format.test.ts");
        let exports = vec![barrel_real_export(&file, "formatCurrency")];
        // The test file must appear in the known-files universe (built
        // from imports/exports) for the match to succeed — a bare
        // internal_import stands in for it importing the module under
        // test.
        let imports = vec![internal_import(&test_file, &file)];
        let issues = run_missing_test_file(imports, exports);
        assert!(
            issues.is_empty(),
            "a same-directory {{name}}.test.ts must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn sibling_tests_dir_test_file_suppresses_finding() {
        let file = PathBuf::from("/p/format.ts");
        let test_file = PathBuf::from("/p/__tests__/format.test.ts");
        let exports = vec![barrel_real_export(&file, "formatCurrency")];
        let imports = vec![internal_import(&test_file, &file)];
        let issues = run_missing_test_file(imports, exports);
        assert!(
            issues.is_empty(),
            "a sibling __tests__/{{name}}.test.ts must suppress the finding; got {issues:?}"
        );
    }

    #[test]
    fn barrel_only_file_is_not_flagged() {
        let file = PathBuf::from("/p/index.ts");
        let exports = vec![barrel_reexport(&file, "x")];
        let issues = run_missing_test_file(vec![], exports);
        assert!(
            issues.is_empty(),
            "a pure re-export barrel has no behavior of its own to test; got {issues:?}"
        );
    }

    #[test]
    fn type_only_export_is_not_flagged() {
        let file = PathBuf::from("/p/types.ts");
        let exports = vec![ExportRecord {
            file: file.clone(),
            name: "Options".to_string(),
            kind: ExportKind::Named,
            type_only: true,
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
            source_specifier: None,
            resolved_source: None,
        }];
        let issues = run_missing_test_file(vec![], exports);
        assert!(
            issues.is_empty(),
            "a type-only export has no runtime behavior to test; got {issues:?}"
        );
    }

    #[test]
    fn test_file_itself_is_exempt() {
        let file = PathBuf::from("/p/format.test.ts");
        let exports = vec![barrel_real_export(&file, "helper")];
        let issues = run_missing_test_file(vec![], exports);
        assert!(
            issues.is_empty(),
            "a test file itself must not be flagged for missing its own test; got {issues:?}"
        );
    }

    #[test]
    fn framework_entry_point_is_exempt() {
        let file = PathBuf::from("/p/app/page.tsx");
        let exports = vec![barrel_real_export(&file, "default")];
        let issues = run_missing_test_file(vec![], exports);
        assert!(
            issues.is_empty(),
            "a framework entry point must be exempt; got {issues:?}"
        );
    }

    fn run_missing_test_file_with_default_patterns(
        imports: Vec<ImportRecord>,
        exports: Vec<ExportRecord>,
    ) -> Vec<CoreIssue> {
        compute_missing_test_files(
            &imports,
            &exports,
            &Default::default(),
            &owned(missing_test_file::DEFAULT_TEST_MATCH_PATTERNS),
            &owned(MTF_TEST_FILE_PATTERNS),
            &owned(MTF_FRAMEWORK_PATTERNS),
        )
    }

    #[test]
    fn default_patterns_cover_js_jsx_mjs_mts_cts_same_directory_test_files() {
        for ext in ["js", "jsx", "mjs", "mts", "cts"] {
            for naming in [".test.", ".spec."] {
                let file = PathBuf::from(format!("/p/format.{ext}"));
                let test_file = PathBuf::from(format!("/p/format{naming}{ext}"));
                let exports = vec![barrel_real_export(&file, "formatCurrency")];
                let imports = vec![internal_import(&test_file, &file)];
                let issues = run_missing_test_file_with_default_patterns(imports, exports);
                assert!(
                    issues.is_empty(),
                    "a same-directory format{naming}{ext} must suppress the finding by default; \
                     got {issues:?}"
                );
            }
        }
    }

    #[test]
    fn default_patterns_cover_js_jsx_mjs_mts_cts_tests_dir_test_files() {
        for ext in ["js", "jsx", "mjs", "mts", "cts"] {
            for naming in [".test.", ".spec."] {
                let file = PathBuf::from(format!("/p/format.{ext}"));
                let test_file = PathBuf::from(format!("/p/__tests__/format{naming}{ext}"));
                let exports = vec![barrel_real_export(&file, "formatCurrency")];
                let imports = vec![internal_import(&test_file, &file)];
                let issues = run_missing_test_file_with_default_patterns(imports, exports);
                assert!(
                    issues.is_empty(),
                    "a __tests__/format{naming}{ext} must suppress the finding by default; got \
                     {issues:?}"
                );
            }
        }
    }

    #[test]
    fn js_source_with_no_matching_test_file_is_still_flagged_by_default() {
        let file = PathBuf::from("/p/uncovered.mjs");
        let exports = vec![barrel_real_export(&file, "uncovered")];
        let issues = run_missing_test_file_with_default_patterns(vec![], exports);
        assert_eq!(
            issues.len(),
            1,
            "a .mjs source with no matching test file must still be flagged by default; got {issues:?}"
        );
        assert_eq!(issues[0].file, file);
    }

    // ─── Design.MissingTestFile: instanceof-only usage (CD-146) ─────────

    fn asserted(
        entries: &[(&Path, &[&str])],
    ) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
        entries
            .iter()
            .map(|(file, names)| {
                (
                    cofferdam_core::path_key(file),
                    names.iter().map(|n| n.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn type_asserted_class_from_a_differently_named_test_is_not_flagged() {
        let errors = PathBuf::from("/p/errors.ts");
        let test_file = PathBuf::from("/p/validate.test.ts");
        let exports = vec![barrel_real_export(&errors, "ValidationError")];
        let imports = vec![named_import(
            &test_file,
            &errors,
            "ValidationError",
            "ValidationError",
        )];
        let issues = run_missing_test_file_asserting(
            imports,
            exports,
            asserted(&[(&test_file, &["ValidationError"])]),
        );
        assert!(
            issues.is_empty(),
            "a class exercised via instanceof/toThrow from validate.test.ts must count as tested; \
             got {issues:?}"
        );
    }

    #[test]
    fn plain_import_from_a_test_file_does_not_count_as_tested() {
        // The discriminator for CD-146: importing a helper into a test
        // to build fixtures is not the same as testing it.
        let helpers = PathBuf::from("/p/helpers.ts");
        let test_file = PathBuf::from("/p/validate.test.ts");
        let exports = vec![barrel_real_export(&helpers, "makeUser")];
        let imports = vec![named_import(&test_file, &helpers, "makeUser", "makeUser")];
        let issues = run_missing_test_file_asserting(imports, exports, Default::default());
        assert_eq!(
            issues.len(),
            1,
            "a plain (non-type-asserted) import from a test must still be flagged; got {issues:?}"
        );
        assert_eq!(issues[0].file, helpers);
    }

    #[test]
    fn type_assertion_of_an_unrelated_name_does_not_suppress() {
        let errors = PathBuf::from("/p/errors.ts");
        let test_file = PathBuf::from("/p/validate.test.ts");
        let exports = vec![barrel_real_export(&errors, "ValidationError")];
        let imports = vec![named_import(
            &test_file,
            &errors,
            "ValidationError",
            "ValidationError",
        )];
        let issues = run_missing_test_file_asserting(
            imports,
            exports,
            asserted(&[(&test_file, &["SomeOtherError"])]),
        );
        assert_eq!(
            issues.len(),
            1,
            "asserting on a different class must not credit this file; got {issues:?}"
        );
    }

    #[test]
    fn type_asserted_class_reached_through_a_barrel_is_not_flagged() {
        let errors = PathBuf::from("/p/errors/validation.ts");
        let barrel = PathBuf::from("/p/errors/index.ts");
        let test_file = PathBuf::from("/p/validate.test.ts");
        let mut forward = barrel_reexport(&barrel, "ValidationError");
        forward.resolved_source = Some(errors.clone());
        let exports = vec![barrel_real_export(&errors, "ValidationError"), forward];
        let imports = vec![named_import(
            &test_file,
            &barrel,
            "ValidationError",
            "ValidationError",
        )];
        let issues = run_missing_test_file_asserting(
            imports,
            exports,
            asserted(&[(&test_file, &["ValidationError"])]),
        );
        assert!(
            issues.is_empty(),
            "a class type-asserted through an errors/index.ts barrel must credit the defining \
             file; got {issues:?}"
        );
    }

    #[test]
    fn run_collects_instanceof_and_matcher_names_from_test_files_only() {
        const TEST_SRC: &str = r#"
import { ValidationError, NotFound, Other } from "./errors";
test("a", () => {
  const e: unknown = new ValidationError();
  expect(e instanceof ValidationError).toBe(true);
  expect(() => { throw new NotFound(); }).toThrow(NotFound);
  expect(new Other()).toBeInstanceOf(Other);
});
"#;
        let mut raw = std::collections::BTreeMap::new();
        raw.insert(
            "test_file_patterns".to_string(),
            RawOptionValue::List(vec![RawOptionValue::String(".test.".to_string())]),
        );
        let meta = MissingTestFile.meta();
        let opts = validate_options(meta.id, meta.options, &raw).expect("valid options");

        let mut collected = Vec::new();
        for path in ["/p/validate.test.ts", "/p/validate.ts"] {
            let corpus = CorpusIndex::new();
            let file = SourceFile::new(PathBuf::from(path), TEST_SRC.to_string());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus)
                .with_options(&opts);
            MissingTestFile.run(&file, &mut ctx);
            collected.push(
                corpus.with_slot(&missing_test_file::TYPE_ASSERTED_NAMES, |slot| slot.clone()),
            );
        }

        let from_test = collected[0]
            .get(&cofferdam_core::path_key(Path::new("/p/validate.test.ts")))
            .cloned()
            .unwrap_or_default();
        let mut names: Vec<&str> = from_test.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["NotFound", "Other", "ValidationError"],
            "instanceof, toThrow, and toBeInstanceOf arguments must all be collected"
        );
        assert!(
            collected[1].is_empty(),
            "a non-test file must contribute nothing; got {:?}",
            collected[1]
        );
    }

    // ─── Design.DuplicateExportName (CD-148) ────────────────────────────

    use cofferdam_core::{validate_options, CheckOptions, RawOptionValue};
    use duplicate_export_name::DuplicateExportName;

    fn duplicate_export_opts(pairs: &[&str]) -> CheckOptions {
        let mut raw: std::collections::BTreeMap<String, RawOptionValue> =
            std::collections::BTreeMap::new();
        raw.insert(
            "exempt_boundary_pairs".to_string(),
            RawOptionValue::List(
                pairs
                    .iter()
                    .map(|p| RawOptionValue::String(p.to_string()))
                    .collect(),
            ),
        );
        let meta = DuplicateExportName.meta();
        validate_options(meta.id, meta.options, &raw).expect("valid options")
    }

    /// Runs `DuplicateExportName.run()` over each `(path, source)` fixture,
    /// then `.finalize()` with `exempt_boundary_pairs` set to `pairs`.
    fn run_duplicate_export_name(fixtures: &[(&Path, &str)], pairs: &[&str]) -> Vec<CoreIssue> {
        let corpus = CorpusIndex::new();
        for (path, src) in fixtures {
            let file = SourceFile::new(path.to_path_buf(), src.to_string());
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            DuplicateExportName.run(&file, &mut ctx);
        }
        let opts = duplicate_export_opts(pairs);
        let mut finalize_ctx = FinalizeContext::new(&corpus).with_options(&opts);
        DuplicateExportName.finalize(&mut finalize_ctx)
    }

    const MIRROR_A: &str = "export const UserId = 1;";
    const MIRROR_B: &str = "export const UserId = 2;";

    #[test]
    fn cross_boundary_mirror_is_flagged_without_config() {
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let issues = run_duplicate_export_name(&[(&client, MIRROR_A), (&server, MIRROR_B)], &[]);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
    }

    #[test]
    fn declared_boundary_pair_is_exempt() {
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let issues = run_duplicate_export_name(
            &[(&client, MIRROR_A), (&server, MIRROR_B)],
            &["client/**|server/**"],
        );
        assert!(
            issues.is_empty(),
            "a declared client/server mirror must be exempt; got {issues:?}"
        );
    }

    #[test]
    fn two_files_on_the_same_side_are_still_flagged() {
        let a = PathBuf::from("/p/client/a.ts");
        let b = PathBuf::from("/p/client/b.ts");
        let issues =
            run_duplicate_export_name(&[(&a, MIRROR_A), (&b, MIRROR_B)], &["client/**|server/**"]);
        assert_eq!(
            issues.len(),
            1,
            "a collision inside one side of the boundary is not a mirror; got {issues:?}"
        );
    }

    #[test]
    fn a_third_file_outside_the_boundary_un_exempts_the_group() {
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let stray = PathBuf::from("/p/utils/misc.ts");
        let issues = run_duplicate_export_name(
            &[
                (&client, MIRROR_A),
                (&server, MIRROR_B),
                (&stray, "export const UserId = 3;"),
            ],
            &["client/**|server/**"],
        );
        assert_eq!(
            issues.len(),
            1,
            "an occurrence outside the declared boundary must keep the finding; got {issues:?}"
        );
    }

    #[test]
    fn an_unrelated_collision_is_unaffected_by_the_exemption() {
        let client = PathBuf::from("/p/client/schema.ts");
        let stray = PathBuf::from("/p/utils/misc.ts");
        let issues = run_duplicate_export_name(
            &[(&client, MIRROR_A), (&stray, MIRROR_B)],
            &["client/**|server/**"],
        );
        assert_eq!(
            issues.len(),
            1,
            "only the declared pairing is exempt; got {issues:?}"
        );
    }

    #[test]
    fn boundary_globs_match_at_any_depth() {
        let client = PathBuf::from("/p/packages/app/client/schema.ts");
        let server = PathBuf::from("/p/packages/app/server/schema.ts");
        let issues = run_duplicate_export_name(
            &[(&client, MIRROR_A), (&server, MIRROR_B)],
            &["client/**|server/**"],
        );
        assert!(
            issues.is_empty(),
            "an unanchored side glob must match nested paths; got {issues:?}"
        );
    }

    #[test]
    fn a_three_sided_boundary_exempts_a_three_way_mirror() {
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let shared = PathBuf::from("/p/shared/schema.ts");
        let issues = run_duplicate_export_name(
            &[
                (&client, MIRROR_A),
                (&server, MIRROR_B),
                (&shared, "export const UserId = 3;"),
            ],
            &["client/**|server/**|shared/**"],
        );
        assert!(
            issues.is_empty(),
            "a three-sided boundary must exempt a three-way mirror; got {issues:?}"
        );
    }

    #[test]
    fn invalid_boundary_glob_never_exempts_and_emits_a_warning() {
        // `client/[` is an unclosed character class — not a valid glob.
        // CD-150: previously `Side::new` just skipped the non-compiling
        // matcher with no diagnostic, so the exemption silently never
        // applied and the user had no way to know why. Now: the
        // collision is still flagged (the broken side matches nothing)
        // AND a warning names the bad pattern.
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let issues = run_duplicate_export_name(
            &[(&client, MIRROR_A), (&server, MIRROR_B)],
            &["client/[|server/**"],
        );
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.message.contains("is exported from"))
                .count(),
            1,
            "the broken side must never match, so the collision stays flagged; got {issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("invalid glob pattern")
                    && i.message.contains("client/[")),
            "expected a warning naming the invalid pattern `client/[`; got {issues:?}"
        );
    }

    #[test]
    fn all_valid_boundary_globs_emit_no_warning() {
        let client = PathBuf::from("/p/client/schema.ts");
        let server = PathBuf::from("/p/server/schema.ts");
        let issues = run_duplicate_export_name(
            &[(&client, MIRROR_A), (&server, MIRROR_B)],
            &["client/**|server/**"],
        );
        assert!(
            !issues
                .iter()
                .any(|i| i.message.contains("invalid glob pattern")),
            "well-formed globs must not emit a warning; got {issues:?}"
        );
    }
}
