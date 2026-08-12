use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    Priority, Severity, SourceFile, Span,
};

use super::package_entry_point::{is_package_entry_point, EntryPointCache};

/// Below this many non-excluded files in the project, mean/stddev are
/// statistically meaningless — skip entirely.
const MIN_FILES: usize = 8;

/// A file's fan-in/fan-out must exceed `mean + STDDEV_MULTIPLIER * stddev`
/// to count as an outlier.
const STDDEV_MULTIPLIER: f64 = 3.0;

/// A metric must also clear this absolute count, not just the relative
/// stddev threshold. In a project whose files import mainly from
/// node_modules the in-project graph is sparse enough that
/// `mean + 3*stddev` falls below 1, and a file importing a single local
/// module would otherwise read as an outlier.
const MIN_ABSOLUTE_FAN: u32 = 8;

/// Basenames treated as intentional aggregator hubs — always high
/// fan-in/fan-out by design (a barrel `index.ts`, a shared `types.ts`).
/// Excluded from BOTH the flaggable set AND the statistical population,
/// since including their legitimately extreme counts would skew the
/// mean/stddev for every other file.
const HUB_BASENAMES: &[&str] = &[
    "index.ts",
    "index.tsx",
    "index.js",
    "index.jsx",
    "index.mjs",
    "index.cjs",
    "types.ts",
    "types.tsx",
];

fn is_hub_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| HUB_BASENAMES.contains(&n))
}

fn is_node_modules_path(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "node_modules")
}

/// Every analyzed file's path, recorded during `run()`. The import/export
/// graph alone only surfaces files that appear as an import source, an
/// export site, or a resolved import target — a file with neither (a
/// side-effect-only script, `export {}`) would otherwise never enter the
/// statistical population even though its fan-in/fan-out of 0 is a
/// legitimate data point.
static ALL_FILES: CorpusKey<HashSet<PathBuf>> =
    CorpusKey::new("Design.ImportFanOutOutlier.all_files");

const META: CheckMeta = CheckMeta {
    id: "Design.ImportFanOutOutlier",
    category: Category::Design,
    base_priority: 6,
    default_severity: Severity::Medium,
    explanation: "A file's import fan-in or fan-out is a statistical outlier versus the rest of \
        the project — a likely \"god module\" (doing too much) or over-centralized dependency \
        (too many things depend on one module).",
    body: include_str!("../../docs/Design.ImportFanOutOutlier.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes every analyzed file's path into ALL_FILES during run(); a
    // cache-hit skip of run() would drop that file from the population
    // (CD-133), mirroring Design.DuplicateTypeShape's pure_run rationale.
    pure_run: false,
};

/// `Design.ImportFanOutOutlier` — finalize-stage check (CD-130) that
/// flags a file whose import fan-out (files it imports) is more than
/// `STDDEV_MULTIPLIER` standard deviations above the project mean AND
/// at least `MIN_ABSOLUTE_FAN`, computed over the project's in-project
/// (resolved) import edges only — external package imports don't count
/// toward either metric. Fan-in (files that import it) is never
/// sufficient on its own; when it clears the same two bars alongside
/// fan-out, the file is reported once as a combined hub finding rather
/// than twice.
///
/// Scope: files matching `HUB_BASENAMES` (barrel `index.*`, `types.*`)
/// or resolving as the nearest `package.json`'s declared entry point
/// (CD-133, same resolution `Design.BarrelReexportBloat` uses) are
/// excluded entirely, both from being flagged and from the statistical
/// population, since a real aggregator's/entry-point's legitimately
/// high count would otherwise inflate the mean/stddev for every other
/// file. A differently-named central hub not covered by either
/// exclusion (e.g. an internal `container.ts` never referenced from
/// `package.json`) may still be flagged. Below `MIN_FILES` non-excluded
/// files, or when a metric's stddev is 0 (every file has the same
/// count), nothing is flagged for that metric.
pub struct ImportFanOutOutlier;

impl Check for ImportFanOutOutlier {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&ALL_FILES, |slot, path| {
            slot.remove(path);
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        ctx.corpus.with_slot(&ALL_FILES, |slot| {
            slot.insert(file.path.clone());
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let imports: std::sync::Arc<Vec<ImportRecord>> =
            ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.to_vec());
        let exports: std::sync::Arc<Vec<ExportRecord>> =
            ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.to_vec());
        let all_files: HashSet<PathBuf> = ctx.corpus.with_slot(&ALL_FILES, |slot| slot.clone());
        compute_outliers(&imports, &exports, &all_files)
    }
}

struct FileStats {
    display: PathBuf,
    fan_in: u32,
    fan_out: u32,
}

fn ensure_entry(by_key: &mut HashMap<String, FileStats>, path: &Path) {
    by_key.entry(path_key(path)).or_insert_with(|| FileStats {
        display: path.to_path_buf(),
        fan_in: 0,
        fan_out: 0,
    });
}

fn mean_stddev(values: &[u32]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / (n - 1.0);
    (mean, variance.sqrt())
}

fn compute_outliers(
    imports: &[ImportRecord],
    exports: &[ExportRecord],
    all_files: &HashSet<PathBuf>,
) -> Vec<Issue> {
    // Universe: any in-project file (mirrors Design.ImportCycle's
    // construction), plus every file recorded via ALL_FILES (CD-133) so a
    // file with neither an import nor an export of its own — invisible to
    // both graph slots — still enters the population with fan-in/fan-out
    // of 0, a legitimate data point rather than an absence.
    let mut by_key: HashMap<String, FileStats> = HashMap::new();
    for path in all_files {
        ensure_entry(&mut by_key, path);
    }
    for imp in imports {
        ensure_entry(&mut by_key, &imp.from_file);
    }
    for exp in exports {
        ensure_entry(&mut by_key, &exp.file);
    }
    for imp in imports {
        let Some(resolved) = &imp.resolved else {
            continue; // external package — doesn't count toward either metric
        };
        // `resolved` is also set for bare specifiers the resolver traced
        // into node_modules (see graph.rs's module doc, which recommends
        // exactly this path-prefix filter) — without it a vendor package
        // with many internal importers shows up as a spurious fan-in
        // outlier and skews the population's mean/stddev.
        if is_node_modules_path(resolved) {
            continue;
        }
        ensure_entry(&mut by_key, resolved);
        by_key.get_mut(&path_key(&imp.from_file)).unwrap().fan_out += 1;
        by_key.get_mut(&path_key(resolved)).unwrap().fan_in += 1;
    }

    // Exclude hub files and package.json-declared entry points from the
    // population entirely — their inclusion would inflate the mean/stddev
    // for every other file. Also exclude node_modules paths defensively
    // at the population level, not just in the edge-counting loop above:
    // a vendor file could otherwise still enter `by_key` via the
    // from_file/export-site seeding loops if it were ever parsed as a
    // source file (e.g. --no-ignore runs).
    let mut entry_point_cache = EntryPointCache::default();
    let population: Vec<&FileStats> = by_key
        .values()
        .filter(|s| {
            !is_hub_file(&s.display)
                && !is_node_modules_path(&s.display)
                && !is_package_entry_point(&s.display, &mut entry_point_cache)
        })
        .collect();
    if population.len() < MIN_FILES {
        return Vec::new();
    }

    let fan_in_values: Vec<u32> = population.iter().map(|s| s.fan_in).collect();
    let fan_out_values: Vec<u32> = population.iter().map(|s| s.fan_out).collect();
    let (mean_in, stddev_in) = mean_stddev(&fan_in_values);
    let (mean_out, stddev_out) = mean_stddev(&fan_out_values);
    let threshold_in = mean_in + STDDEV_MULTIPLIER * stddev_in;
    let threshold_out = mean_out + STDDEV_MULTIPLIER * stddev_out;

    let mut issues = Vec::new();
    let zero_span = Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    };
    let mut sorted_population = population;
    sorted_population.sort_by(|a, b| a.display.cmp(&b.display));
    for stats in sorted_population {
        let fan_in_is_outlier = stddev_in > 0.0
            && stats.fan_in as f64 > threshold_in
            && stats.fan_in >= MIN_ABSOLUTE_FAN;
        let fan_out_is_outlier = stddev_out > 0.0
            && stats.fan_out as f64 > threshold_out
            && stats.fan_out >= MIN_ABSOLUTE_FAN;
        // Exclusive branches: fan-in never qualifies without fan-out also
        // qualifying (see the module doc), so at most one issue per file
        // — a combined finding when both are outliers, else a fan-out-only
        // finding, never both for the same file.
        if fan_in_is_outlier && fan_out_is_outlier {
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "unusually high import fan-in and fan-out: {} files import this one and it imports {} others (project mean fan-in {mean_in:.1}, stddev {stddev_in:.1}) — possible over-centralized hub",
                    stats.fan_in, stats.fan_out
                ),
                file: stats.display.clone(),
                location: Location::from_span(&stats.display, zero_span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        } else if fan_out_is_outlier {
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "unusually high import fan-out: this file imports {} others (project mean {mean_out:.1}, stddev {stddev_out:.1}) — possible \"god module\"",
                    stats.fan_out
                ),
                file: stats.display.clone(),
                location: Location::from_span(&stats.display, zero_span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::graph::{ImportKind, ImportedName};
    use std::path::PathBuf;

    fn edge(from: &Path, to: &Path) -> ImportRecord {
        ImportRecord {
            from_file: from.to_path_buf(),
            source_specifier: "./m".to_string(),
            resolved: Some(to.to_path_buf()),
            names: vec![ImportedName {
                source_name: "x".to_string(),
                local_name: "x".to_string(),
                kind: ImportKind::Named,
                type_only: false,
                local_use_count: 1,
            }],
            type_only: false,
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
        }
    }

    fn messages_for<'a>(issues: &'a [Issue], needle: &str) -> Vec<&'a Issue> {
        issues
            .iter()
            .filter(|i| i.message.contains(needle))
            .collect()
    }

    /// CD-333 regression: a leaf utility with many importers but no
    /// imports of its own (fan-in high, fan-out low) must not be
    /// flagged — it is a shared module doing its job, not a hub.
    #[test]
    fn high_fan_in_alone_is_not_flagged() {
        let leaf = PathBuf::from("/proj/leaf.ts");
        let mut all_files = HashSet::new();
        all_files.insert(leaf.clone());
        let mut imports = Vec::new();
        for i in 0..15 {
            let importer = PathBuf::from(format!("/proj/importer_{i}.ts"));
            all_files.insert(importer.clone());
            imports.push(edge(&importer, &leaf));
        }

        let issues = compute_outliers(&imports, &[], &all_files);
        assert!(
            issues.is_empty(),
            "expected no findings for a high-fan-in-only leaf, got {issues:?}"
        );
    }

    /// A genuine god module — high fan-in AND high fan-out — must
    /// still fire the fan-in finding.
    #[test]
    fn high_fan_in_and_fan_out_together_is_flagged() {
        let god = PathBuf::from("/proj/god.ts");
        let mut all_files = HashSet::new();
        all_files.insert(god.clone());
        let mut imports = Vec::new();
        for i in 0..15 {
            let sibling = PathBuf::from(format!("/proj/sibling_{i}.ts"));
            all_files.insert(sibling.clone());
            imports.push(edge(&sibling, &god)); // sibling -> god: god's fan-in
            imports.push(edge(&god, &sibling)); // god -> sibling: god's fan-out
        }

        let issues = compute_outliers(&imports, &[], &all_files);
        let fan_in_hits = messages_for(&issues, "fan-in");
        assert_eq!(
            fan_in_hits.len(),
            1,
            "expected exactly one fan-in finding, got {issues:?}"
        );
        assert_eq!(fan_in_hits[0].file, god);
    }

    /// The fan-out branch is untouched: high fan-out with low fan-in
    /// still produces exactly the fan-out finding, nothing else for
    /// that file.
    #[test]
    fn high_fan_out_alone_is_still_flagged() {
        let hub = PathBuf::from("/proj/hub.ts");
        let mut all_files = HashSet::new();
        all_files.insert(hub.clone());
        let mut imports = Vec::new();
        for i in 0..15 {
            let target = PathBuf::from(format!("/proj/target_{i}.ts"));
            all_files.insert(target.clone());
            imports.push(edge(&hub, &target));
        }

        let issues = compute_outliers(&imports, &[], &all_files);
        let hub_issues: Vec<&Issue> = issues.iter().filter(|i| i.file == hub).collect();
        assert_eq!(
            hub_issues.len(),
            1,
            "expected exactly one finding for hub, got {issues:?}"
        );
        assert!(hub_issues[0].message.contains("fan-out"));
    }

    /// A leaf imported by many files but itself importing only one local
    /// module must not be flagged: `high_fan_in_alone_is_not_flagged`
    /// above only passes because its leaf has fan_out == 0, which is
    /// unrealistic — a leaf with a single import has a fan_out that can
    /// still clear a sparse graph's relative threshold without an
    /// absolute floor.
    #[test]
    fn sparse_graph_does_not_flag_a_leaf_that_imports_one_module() {
        let leaf = PathBuf::from("/proj/leaf.ts");
        let leaf_dep = PathBuf::from("/proj/leaf_dep.ts");
        let mut all_files = HashSet::new();
        all_files.insert(leaf.clone());
        all_files.insert(leaf_dep.clone());
        let mut imports = Vec::new();
        for i in 0..15 {
            let importer = PathBuf::from(format!("/proj/importer_{i}.ts"));
            all_files.insert(importer.clone());
            imports.push(edge(&importer, &leaf));
        }
        imports.push(edge(&leaf, &leaf_dep));
        for i in 0..150 {
            all_files.insert(PathBuf::from(format!("/proj/filler_{i}.ts")));
        }

        let issues = compute_outliers(&imports, &[], &all_files);
        assert!(
            issues.is_empty(),
            "expected no findings for a leaf with fan_out 1 in a sparse graph, got {issues:?}"
        );
    }

    /// The same file must never receive both a fan-out finding and a
    /// combined fan-in/fan-out finding.
    #[test]
    fn a_file_is_never_reported_twice() {
        let hub = PathBuf::from("/proj/hub.ts");
        let mut all_files = HashSet::new();
        all_files.insert(hub.clone());
        let mut imports = Vec::new();
        for i in 0..15 {
            let sibling = PathBuf::from(format!("/proj/sibling_{i}.ts"));
            all_files.insert(sibling.clone());
            imports.push(edge(&sibling, &hub));
            imports.push(edge(&hub, &sibling));
        }

        let issues = compute_outliers(&imports, &[], &all_files);
        let hub_issues: Vec<&Issue> = issues.iter().filter(|i| i.file == hub).collect();
        assert_eq!(
            hub_issues.len(),
            1,
            "expected exactly one issue for hub, got {issues:?}"
        );
    }
}
