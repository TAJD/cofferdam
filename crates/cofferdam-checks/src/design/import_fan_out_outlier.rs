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
/// flags a file whose import fan-in (files that import it) or fan-out
/// (files it imports) is more than `STDDEV_MULTIPLIER` standard
/// deviations above the project mean, computed over the project's
/// in-project (resolved) import edges only — external package imports
/// don't count toward either metric.
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
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
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
        if stddev_in > 0.0 && stats.fan_in as f64 > threshold_in {
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "unusually high import fan-in: {} files import this one (project mean {mean_in:.1}, stddev {stddev_in:.1}) — possible over-centralized dependency",
                    stats.fan_in
                ),
                file: stats.display.clone(),
                location: Location::from_span(&stats.display, zero_span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
        if stddev_out > 0.0 && stats.fan_out as f64 > threshold_out {
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
