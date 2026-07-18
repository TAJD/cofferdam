use std::collections::HashSet;
use std::path::Path;

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, OptionDefault,
    OptionKind, OptionSpec, Priority, Severity, SourceFile,
};

/// `{name}` template patterns checked (relative to the source file's own
/// directory) for a corresponding test file. `{name}` substitutes the
/// source file's stem (filename without extension).
const DEFAULT_TEST_MATCH_PATTERNS: &[&str] = &[
    "{name}.test.ts",
    "{name}.test.tsx",
    "{name}.spec.ts",
    "{name}.spec.tsx",
    "__tests__/{name}.test.ts",
    "__tests__/{name}.test.tsx",
    "__tests__/{name}.spec.ts",
    "__tests__/{name}.spec.tsx",
];

/// Filename substrings that mark a file as itself a test/mock — mirrors
/// `Design.OrphanExport`'s `test_file_patterns` default. Duplicated
/// rather than shared: the two checks use the list for different
/// purposes (skip exports vs. skip a source file needing its own test)
/// and aren't guaranteed to want to change in lockstep.
const DEFAULT_TEST_FILE_PATTERNS: &[&str] = &[
    ".test.",
    ".spec.",
    "_test.",
    "_spec.",
    "/__tests__/",
    "/__mocks__/",
];

const MTF_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "test_match_patterns",
        kind: OptionKind::StringList,
        default: OptionDefault::StringList(DEFAULT_TEST_MATCH_PATTERNS),
        doc: "Templates checked, relative to the source file's own directory, for a \
            corresponding test file. `{name}` substitutes the source file's stem. A file with \
            no match under any template is flagged.",
    },
    OptionSpec {
        name: "test_file_patterns",
        kind: OptionKind::StringList,
        default: OptionDefault::StringList(DEFAULT_TEST_FILE_PATTERNS),
        doc: "Filename substrings that mark a file as itself a test/mock. Such files are never \
            flagged for missing a test of their own.",
    },
    OptionSpec {
        name: "framework_entry_patterns",
        kind: OptionKind::StringList,
        default: OptionDefault::StringList(FRAMEWORK_ENTRY_PATTERNS),
        doc: "Filename substrings for framework entry-point files (Next.js App Router, Pages \
            Router, SvelteKit, config files). These are exempt — the framework runtime, not a \
            unit test, is what exercises them.",
    },
];

const META: CheckMeta = CheckMeta {
    id: "Design.MissingTestFile",
    category: Category::Design,
    base_priority: 4,
    default_severity: Severity::Low,
    explanation: "A file exports at least one real (non-type-only, non-re-export) symbol but \
        no corresponding test file exists anywhere in the project.",
    body: include_str!("../../docs/Design.MissingTestFile.md"),
    requires_types: false,
    consistency: false,
    options: MTF_OPTIONS,
    autofix: false,
    pure_run: true,
};

/// `Design.MissingTestFile` — finalize-stage check (CD-132) that flags
/// a file with at least one real export (a plain `Named`/`Default`
/// export that isn't type-only) for which no matching test file exists
/// anywhere in the project.
///
/// A file whose only exports are type-only (interfaces, type aliases)
/// or re-exports (`export * from`/`export { x } from` — a barrel) is
/// never a candidate: it has no exported *behavior* of its own to
/// test. A file already recognised as a test/mock itself
/// (`test_file_patterns`) or a framework entry point
/// (`framework_entry_patterns`) is exempt too.
///
/// Scope (v1): the project's "known files" universe is built from the
/// import/export graph (same construction as `Design.ImportFanOutOutlier`)
/// — a test file with zero imports and zero exports of its own (no
/// module under test imported, no re-export) wouldn't appear in that
/// universe and so couldn't satisfy the match even if it exists on
/// disk. In practice a test file almost always imports the module it's
/// testing, so this is a narrow gap.
pub struct MissingTestFile;

impl Check for MissingTestFile {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let test_match_patterns = ctx
            .options
            .get_string_list("test_match_patterns")
            .map(|xs| xs.to_vec())
            .unwrap_or_default();
        let test_file_patterns = ctx
            .options
            .get_string_list("test_file_patterns")
            .map(|xs| xs.to_vec())
            .unwrap_or_default();
        let framework_entry_patterns = ctx
            .options
            .get_string_list("framework_entry_patterns")
            .map(|xs| xs.to_vec())
            .unwrap_or_default();

        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        compute_missing_test_files(
            &imports,
            &exports,
            &test_match_patterns,
            &test_file_patterns,
            &framework_entry_patterns,
        )
    }
}

fn matches_substring(path: &Path, patterns: &[String]) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|p| normalized.contains(p.as_str()))
}

/// Public-by-convention (mirrors `Design.OrphanExport`'s
/// `compute_orphans`) so unit tests can exercise the matching logic
/// directly, without fighting `FinalizeContext::options`' default
/// empty bag.
pub fn compute_missing_test_files(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    test_match_patterns: &[String],
    test_file_patterns: &[String],
    framework_entry_patterns: &[String],
) -> Vec<Issue> {
    // Known-files universe: any file that appears as an import source,
    // an export site, or a resolved import target. A file with neither
    // imports nor exports of its own is invisible here (see CheckMeta
    // doc for the narrow real-world impact).
    let mut known_files: HashSet<String> = HashSet::new();
    for imp in imports {
        known_files.insert(path_key(&imp.from_file));
        if let Some(resolved) = &imp.resolved {
            known_files.insert(path_key(resolved));
        }
    }
    for exp in exports {
        known_files.insert(path_key(&exp.file));
    }

    let mut by_file: std::collections::HashMap<std::path::PathBuf, Vec<&ExportRecord>> =
        std::collections::HashMap::new();
    for exp in exports {
        by_file.entry(exp.file.clone()).or_default().push(exp);
    }

    let mut issues = Vec::new();
    for (file, file_exports) in &by_file {
        if matches_substring(file, test_file_patterns)
            || matches_substring(file, framework_entry_patterns)
        {
            continue;
        }
        let mut real_exports: Vec<&&ExportRecord> = file_exports
            .iter()
            .filter(|e| !e.type_only && matches!(e.kind, ExportKind::Named | ExportKind::Default))
            .collect();
        if real_exports.is_empty() {
            continue;
        }
        real_exports.sort_by_key(|e| e.span.start_byte);

        let Some(dir) = file.parent() else { continue };
        let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let has_test = test_match_patterns.iter().any(|pattern| {
            let candidate_rel = pattern.replace("{name}", stem);
            let candidate = dir.join(&candidate_rel);
            known_files.contains(&path_key(&candidate))
        });
        if has_test {
            continue;
        }

        let anchor = real_exports[0];
        issues.push(Issue {
            check_id: META.id.to_string(),
            message: format!(
                "`{}` is exported here but no matching test file was found in the project",
                anchor.name
            ),
            file: file.clone(),
            location: Location::from_span(file, anchor.span),
            priority: Priority(META.base_priority),
            severity: META.default_severity,
            related: Vec::new(),
        });
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
    });
    issues
}
