use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::framework_paths::FRAMEWORK_ENTRY_PATTERNS;
use cofferdam_core::graph::{
    ExportKind, ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, Severity, SourceFile,
};
use oxc_ast::ast::{BinaryExpression, BinaryOperator, CallExpression, Expression};
use oxc_ast_visit::{walk, Visit};

/// `{name}` template patterns checked (relative to the source file's own
/// directory) for a corresponding test file. `{name}` substitutes the
/// source file's stem (filename without extension).
///
/// Covers `.ts`/`.tsx` as well as `.js`/`.jsx`/`.mjs`/`.mts`/`.cts` sources
/// (CD-137) — patterns aren't matched against the source file's own
/// extension, so a `.ts` source with only a stray `.test.js` sibling would
/// technically also suppress a finding, but that cross-extension case is
/// vanishingly rare in practice.
pub(crate) const DEFAULT_TEST_MATCH_PATTERNS: &[&str] = &[
    "{name}.test.ts",
    "{name}.test.tsx",
    "{name}.test.js",
    "{name}.test.jsx",
    "{name}.test.mjs",
    "{name}.test.mts",
    "{name}.test.cts",
    "{name}.spec.ts",
    "{name}.spec.tsx",
    "{name}.spec.js",
    "{name}.spec.jsx",
    "{name}.spec.mjs",
    "{name}.spec.mts",
    "{name}.spec.cts",
    "__tests__/{name}.test.ts",
    "__tests__/{name}.test.tsx",
    "__tests__/{name}.test.js",
    "__tests__/{name}.test.jsx",
    "__tests__/{name}.test.mjs",
    "__tests__/{name}.test.mts",
    "__tests__/{name}.test.cts",
    "__tests__/{name}.spec.ts",
    "__tests__/{name}.spec.tsx",
    "__tests__/{name}.spec.js",
    "__tests__/{name}.spec.jsx",
    "__tests__/{name}.spec.mjs",
    "__tests__/{name}.spec.mts",
    "__tests__/{name}.spec.cts",
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

/// Jest/Vitest matchers whose *argument* is a class used as a type
/// assertion rather than a value under test. `expect(fn).toThrow(E)`
/// and `expect(e).toBeInstanceOf(E)` exercise `E`'s type identity in
/// exactly the way a bare `instanceof E` does.
const TYPE_ASSERTION_MATCHERS: &[&str] = &["toThrow", "toThrowError", "toBeInstanceOf"];

/// Test file (`path_key`) → local identifier names that file uses in a
/// type-assertion position (`x instanceof Name`, `.toThrow(Name)`,
/// `.toBeInstanceOf(Name)`). Filled during `run` for test files only;
/// read back in `finalize` (CD-146).
pub(crate) static TYPE_ASSERTED_NAMES: CorpusKey<HashMap<String, HashSet<String>>> =
    CorpusKey::new("Design.MissingTestFile.type_asserted_names");

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
    // Writes the per-test-file type-assertion index into the
    // TYPE_ASSERTED_NAMES slot during run(); skipping run() on a
    // findings-cache hit would drop those contributions and resurrect
    // the CD-146 false positive.
    pure_run: false,
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

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // Only test files contribute; every other file skips the walk
        // after two cheap string tests.
        let test_file_patterns = ctx
            .options
            .get_string_list("test_file_patterns")
            .map(|xs| xs.to_vec())
            .unwrap_or_default();
        if !matches_substring(&file.path, &test_file_patterns) {
            return Vec::new();
        }
        if !file.text.contains("instanceof")
            && !TYPE_ASSERTION_MATCHERS
                .iter()
                .any(|m| file.text.contains(m))
        {
            return Vec::new();
        }
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = TypeAssertionCollector {
            names: HashSet::new(),
        };
        visitor.visit_program(parsed.program);
        if !visitor.names.is_empty() {
            let key = path_key(&file.path);
            ctx.corpus.with_slot(&TYPE_ASSERTED_NAMES, |slot| {
                slot.entry(key).or_default().extend(visitor.names.clone());
            });
        }
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
        let type_asserted = ctx
            .corpus
            .with_slot(&TYPE_ASSERTED_NAMES, |slot| slot.clone());
        compute_missing_test_files(
            &imports,
            &exports,
            &type_asserted,
            &test_match_patterns,
            &test_file_patterns,
            &framework_entry_patterns,
        )
    }
}

/// Collects identifiers used in a type-assertion position within a
/// test file: `x instanceof Name`, `.toThrow(Name)`,
/// `.toThrowError(Name)`, `.toBeInstanceOf(Name)`.
struct TypeAssertionCollector {
    names: HashSet<String>,
}

impl<'a> Visit<'a> for TypeAssertionCollector {
    fn visit_binary_expression(&mut self, node: &BinaryExpression<'a>) {
        if node.operator == BinaryOperator::Instanceof {
            if let Expression::Identifier(id) = &node.right {
                self.names.insert(id.name.to_string());
            }
        }
        walk::walk_binary_expression(self, node);
    }

    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &node.callee {
            if TYPE_ASSERTION_MATCHERS.contains(&member.property.name.as_str()) {
                if let Some(Expression::Identifier(id)) =
                    node.arguments.first().and_then(|a| a.as_expression())
                {
                    self.names.insert(id.name.to_string());
                }
            }
        }
        walk::walk_call_expression(self, node);
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
/// Index of export names a test file exercises via a type assertion,
/// keyed by the `path_key` of the file that *defines* the export
/// (CD-146).
///
/// An entry is recorded only when all three line up: a test file
/// imports a binding, that binding's local name appears in a
/// type-assertion position in the same file, and the import resolves
/// to the defining file. Resolution follows one re-export hop so a
/// class reached through an `errors/index.ts` barrel still credits
/// `errors/validation.ts` — the layout error classes almost always
/// use.
fn type_asserted_exports_by_defining_file(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    type_asserted: &HashMap<String, HashSet<String>>,
) -> HashMap<String, HashSet<String>> {
    let mut index: HashMap<String, HashSet<String>> = HashMap::new();
    if type_asserted.is_empty() {
        return index;
    }

    let mut reexports_by_barrel: HashMap<String, Vec<&ExportRecord>> = HashMap::new();
    for exp in exports {
        if exp.kind == ExportKind::ReExport && exp.resolved_source.is_some() {
            reexports_by_barrel
                .entry(path_key(&exp.file))
                .or_default()
                .push(exp);
        }
    }

    for imp in imports {
        let Some(asserted) = type_asserted.get(&path_key(&imp.from_file)) else {
            continue;
        };
        let Some(resolved) = &imp.resolved else {
            continue;
        };
        let resolved_key = path_key(resolved);
        for name in &imp.names {
            if name.type_only || !asserted.contains(&name.local_name) {
                continue;
            }
            index
                .entry(resolved_key.clone())
                .or_default()
                .insert(name.source_name.clone());
            // One re-export hop: the import landed on a barrel that
            // forwards this name from the file that actually defines it.
            for re in reexports_by_barrel
                .get(&resolved_key)
                .into_iter()
                .flatten()
                .filter(|re| re.name == name.source_name || re.name == "*")
            {
                let Some(source) = &re.resolved_source else {
                    continue;
                };
                index
                    .entry(path_key(source))
                    .or_default()
                    .insert(name.source_name.clone());
            }
        }
    }
    index
}

pub fn compute_missing_test_files(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    type_asserted: &HashMap<String, HashSet<String>>,
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

    let type_asserted_index =
        type_asserted_exports_by_defining_file(imports, exports, type_asserted);

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
        // CD-146: a class this file exports is exercised for its type
        // identity (`instanceof` / `toThrow` / `toBeInstanceOf`) by a
        // test file that doesn't happen to be named after this file.
        if type_asserted_index
            .get(&path_key(file))
            .is_some_and(|names| real_exports.iter().any(|e| names.contains(&e.name)))
        {
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
