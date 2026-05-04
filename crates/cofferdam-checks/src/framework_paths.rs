//! Shared framework entry-point pattern list (cd-q53).
//!
//! Filename substrings that mark a file as a framework entry point —
//! Next.js App Router routing/metadata files, Pages Router, SvelteKit,
//! and the usual project-config files (`*.config.ts`). The framework
//! runtime — not application code — consumes these, so graph-aware
//! checks must skip them when reasoning about "is this file's export
//! consumed?".
//!
//! Two checks consume this list today: `Design.OrphanExport` (via its
//! `framework_entry_patterns` option default) and `Warning.UnusedImport`
//! (via the `is_framework_entry` helper). Hoisted into one module so the
//! lists can't drift apart.

use std::path::Path;

pub const FRAMEWORK_ENTRY_PATTERNS: &[&str] = &[
    // Next.js App Router routing files.
    "/page.",
    "/layout.",
    "/route.",
    "/error.",
    "/loading.",
    "/not-found.",
    "/default.",
    "/template.",
    "/global-error.",
    "/middleware.",
    "/instrumentation.",
    // Next.js metadata files (consumed by the framework, not user code).
    "/manifest.",
    "/robots.",
    "/sitemap.",
    "/icon.",
    "/apple-icon.",
    "/opengraph-image.",
    "/twitter-image.",
    "/favicon.",
    // Next.js Pages Router.
    "/_app.",
    "/_document.",
    "/_error.",
    // SvelteKit routing.
    "/+page.",
    "/+layout.",
    "/+server.",
    // Common project-config files.
    "/next.config.",
    "/vite.config.",
    "/vitest.config.",
    "/tsup.config.",
    "/tailwind.config.",
    "/postcss.config.",
    "/astro.config.",
    "/svelte.config.",
    "/remix.config.",
    "/playwright.config.",
    "/jest.config.",
    "/rollup.config.",
    "/webpack.config.",
    "/babel.config.",
    "/eslint.config.",
    "/prettier.config.",
];

/// True iff `path` matches any of `FRAMEWORK_ENTRY_PATTERNS` as a
/// substring of its forward-slash-normalized form. Caller should use
/// this when no per-check option override is in scope; checks that
/// expose a configurable pattern list should compare against the
/// resolved option value instead (see `Design.OrphanExport`).
pub fn is_framework_entry(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let normalized = s.replace('\\', "/");
    FRAMEWORK_ENTRY_PATTERNS
        .iter()
        .any(|p| normalized.contains(p))
}
