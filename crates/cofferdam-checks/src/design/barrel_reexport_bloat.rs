use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_core::graph::{ExportKind, ExportRecord, EXPORTS as GRAPH_EXPORTS};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, FinalizeContext, Issue, Location, Priority, Severity,
    SourceFile, Span,
};

use super::package_entry_point::{is_package_entry_point, EntryPointCache};

/// Below this many eligible barrel candidates in the project, a
/// mean/stddev over their re-export ratios is statistically
/// meaningless — skip entirely. Barrels are rarer than files overall,
/// so this floor is lower than `Design.ImportFanOutOutlier`'s.
const MIN_BARRELS: usize = 5;

/// A barrel's re-export ratio must exceed `mean + STDDEV_MULTIPLIER *
/// stddev` (over other barrels in the project) to count as bloated.
const STDDEV_MULTIPLIER: f64 = 3.0;

const META: CheckMeta = CheckMeta {
    id: "Design.BarrelReexportBloat",
    category: Category::Design,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "A barrel file re-exports an unusually large fraction of its directory's \
        exports versus other barrels in the project — the module's real public surface \
        becomes unclear and tree-shaking is defeated.",
    body: include_str!("../../docs/Design.BarrelReexportBloat.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: true,
};

/// `Design.BarrelReexportBloat` — finalize-stage check (CD-131) that
/// flags a file whose `export * from`/`export { x } from` re-export
/// count, as a fraction of its sibling files' real (non-re-export)
/// exports, is more than `STDDEV_MULTIPLIER` standard deviations above
/// the mean ratio of other barrels in the project.
///
/// Scope (v1): a file that resolves as the project's (or a workspace
/// package's) public entry point — found by walking up from the file
/// to the nearest `package.json` and checking whether the file matches
/// any string reachable from its `main`/`module`/`types`/`exports`
/// fields — is excluded entirely, both from being flagged and from the
/// statistical population, since a real published entry point's high
/// re-export ratio is by design. Entry-point matching compares paths
/// with the file extension stripped, so it only catches package.json
/// fields that point directly at the source file (or a same-directory
/// build artifact); a project with a separate `src`/`dist` split whose
/// `main` points into `dist/` isn't covered and may be flagged.
pub struct BarrelReexportBloat;

impl Check for BarrelReexportBloat {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let exports: Vec<ExportRecord> = ctx.corpus.with_slot(&GRAPH_EXPORTS, |slot| slot.clone());
        compute_bloated_barrels(&exports)
    }
}

#[derive(Default)]
struct FileExportCounts {
    real: u32,
    reexport: u32,
}

fn mean_stddev(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values
        .iter()
        .map(|&v| {
            let d = v - mean;
            d * d
        })
        .sum::<f64>()
        / (n - 1.0);
    (mean, variance.sqrt())
}

fn compute_bloated_barrels(exports: &[ExportRecord]) -> Vec<Issue> {
    let mut by_file: HashMap<PathBuf, FileExportCounts> = HashMap::new();
    for exp in exports {
        let counts = by_file.entry(exp.file.clone()).or_default();
        match exp.kind {
            ExportKind::ReExport => counts.reexport += 1,
            ExportKind::Named | ExportKind::Default => counts.real += 1,
        }
    }

    // Directory scoping below compares raw `PathBuf` parents rather than
    // `path_key`-normalising them first, unlike `ImportFanOutOutlier`.
    // This is sound only because every `ExportRecord.file` here is the
    // single discovery-walk path (`file.path` in the engine's graph
    // pass) and never a resolver-produced path — one consistent
    // spelling in, one consistent spelling out. If a future change ever
    // populates `ExportRecord.file` from a resolved/joined path instead,
    // this comparison would need `path_key` too.
    let mut entry_point_cache: EntryPointCache = HashMap::new();
    struct Candidate {
        file: PathBuf,
        ratio: f64,
    }
    let mut candidates: Vec<Candidate> = Vec::new();

    for (file, counts) in &by_file {
        if counts.reexport == 0 {
            continue;
        }
        if is_package_entry_point(file, &mut entry_point_cache) {
            continue;
        }
        let Some(dir) = file.parent() else { continue };
        // A directory whose only files are barrels re-exporting entire
        // subdirectories (no local sibling file with a "real" export)
        // has `sibling_real == 0` and is skipped here — arguably the
        // most extreme mega-barrel shape, but the ratio is undefined
        // without a sibling denominator. Often these are also the
        // package's own entry point (already excluded above), which
        // softens the gap; accepted v1 scope otherwise.
        let sibling_real: u32 = by_file
            .iter()
            .filter(|(other, _)| other.as_path() != file.as_path() && other.parent() == Some(dir))
            .map(|(_, c)| c.real)
            .sum();
        if sibling_real == 0 {
            continue;
        }
        candidates.push(Candidate {
            file: file.clone(),
            ratio: counts.reexport as f64 / sibling_real as f64,
        });
    }

    if candidates.len() < MIN_BARRELS {
        return Vec::new();
    }

    let ratios: Vec<f64> = candidates.iter().map(|c| c.ratio).collect();
    let (mean, stddev) = mean_stddev(&ratios);
    if stddev <= 0.0 {
        return Vec::new();
    }
    let threshold = mean + STDDEV_MULTIPLIER * stddev;

    let mut sorted = candidates;
    sorted.sort_by(|a, b| a.file.cmp(&b.file));

    let zero_span = Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    };
    sorted
        .into_iter()
        .filter(|c| c.ratio > threshold)
        .map(|c| Issue {
            check_id: META.id.to_string(),
            message: format!(
                "this barrel re-exports a much larger share of its directory's exports ({:.0}%) than other barrels in the project (mean {:.0}%, stddev {:.0}%) \u{2014} its real public surface is unclear and tree-shaking is defeated",
                c.ratio * 100.0,
                mean * 100.0,
                stddev * 100.0
            ),
            file: c.file.clone(),
            location: Location::from_span(&c.file, zero_span),
            priority: Priority(META.base_priority),
            severity: Severity::Medium,
            related: Vec::new(),
        })
        .collect()
}
