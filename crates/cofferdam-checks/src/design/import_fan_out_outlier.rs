use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{
    ExportRecord, ImportRecord, EXPORTS as GRAPH_EXPORTS, IMPORTS as GRAPH_IMPORTS,
};
use cofferdam_core::path_key;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, Priority, Severity,
    SourceFile, Span,
};

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
    pure_run: true,
};

/// `Design.ImportFanOutOutlier` — finalize-stage check (CD-130) that
/// flags a file whose import fan-in (files that import it) or fan-out
/// (files it imports) is more than `STDDEV_MULTIPLIER` standard
/// deviations above the project mean, computed over the project's
/// in-project (resolved) import edges only — external package imports
/// don't count toward either metric.
///
/// Scope (v1): files matching `HUB_BASENAMES` (barrel `index.*`,
/// `types.*`) are excluded entirely, both from being flagged and from
/// the statistical population, since a real aggregator's legitimately
/// high count would otherwise inflate the mean/stddev for every other
/// file. A project with a differently-named central hub (e.g.
/// `container.ts`) isn't covered by this exclusion and may be flagged.
/// Below `MIN_FILES` non-hub files, or when a metric's stddev is 0
/// (every file has the same count), nothing is flagged for that metric.
pub struct ImportFanOutOutlier;

impl Check for ImportFanOutOutlier {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        compute_outliers(&imports, &exports)
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

fn compute_outliers(imports: &[ImportRecord], exports: &[ExportRecord]) -> Vec<Issue> {
    // Universe: any in-project file (mirrors Design.ImportCycle's
    // construction) — every file that either made an import or exports
    // something. Files with zero fan-in and zero fan-out are still part
    // of the population (their count is legitimately 0).
    let mut by_key: HashMap<String, FileStats> = HashMap::new();
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

    // Exclude hub files from the population entirely — their inclusion
    // would inflate the mean/stddev for every other file.
    let population: Vec<&FileStats> = by_key
        .values()
        .filter(|s| !is_hub_file(&s.display))
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
