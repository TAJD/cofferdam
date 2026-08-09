# Language support

## TypeScript (primary)

TypeScript is cofferdam's primary surface: every flagship check — orphan exports, layering, complexity, suppression hygiene — targets TS / TSX / JS / JSX / MJS / CJS via the [oxc](https://oxc.rs/) parser.

## HTML (build-output / plugin surface)

An HTML adapter ships under `crates/cofferdam-html`, built on
tree-sitter-html. It backs two entry points: built-in and plugin checks can
target `.html` files directly under `cofferdam check` (`files: {
extensions: ["html"] }` for plugins), and `.html`/`.htm` files under a
built output directory via [`cofferdam verify --dist`](/verify-dist). The
plugin `AstView`/`LineView` surface is the same shape used for TypeScript —
`findAll("Element")`, `.attributes`, `.children` — documented in the
[Author guide](/plugin-sdk-guide#html-findall-—-flag-img-with-no-alt-cd-84)
and demonstrated end-to-end in [SEO-grade checking](/seo-checking).

## Markdown / MDX (plugin surface, opt-in)

`.md`/`.mdx` files aren't discovered by default — walking every README and
changelog in a repo on every run would inflate the file walk for projects
that never asked for it. A project that wants plugin checks over text
content (blog posts, docs) opts in per-project:

```toml
# cofferdam.toml
[engine]
extra_extensions = ["md", "mdx"]
```

Markdown files get no whole-file parse (`file.ast` is `null` on the plugin
surface, matching Astro's non-goal), but `file.text` and line-scan
`LineView`s are populated, so Pattern-A regex/line-based plugin checks
(frontmatter length, heading structure, link density) work the same way
they do for `.astro`. No built-in check targets Markdown — this is a
plugin-only surface.

## Rust (second language, dogfood)

A Rust adapter ships under `crates/cofferdam-rust`, exercising the engine's per-language dispatch (cd-91zc). Three checks today:

- `Rust.NoUnwrapInLib` — no `.unwrap()` in library code
- `Rust.NoUnimplementedInNonTest` — no `unimplemented!()` outside tests
- `Rust.MissingPubDoc` — public items need doc comments

A CI dogfood job runs cofferdam against its own Rust workspace on every PR.

The Rust adapter is load-bearing for the polylingual architecture pledge: until a second domain existed, "framework, not TS-tool" was theory. It now ships as a working second parser feeding the same `Check` trait the TS adapter uses.

## Typst (package linting, separate subcommand)

`cofferdam typst <dir>` lints a [Typst](https://typst.app) package
directory bound for [Typst Universe](https://typst.app/universe/). It is a
subcommand rather than an adapter because its unit of analysis is a
*directory* — `typst.toml`, `LICENSE`, `README.md` and the published
bundle together — which does not fit the engine's per-file, AST-oriented
`Check` trait. The crate (`crates/cofferdam-typst`) defines its own
`TypstCheck` trait and reuses `cofferdam_core::Issue`, so findings render
through the same formatters as everything else.

```sh
cofferdam typst .                # lint the package in the current directory
cofferdam typst pkg/ --robot     # JSON, for CI
```

Eleven checks ship today, all keyed to Universe's submission rules:

| Check | Catches |
|---|---|
| `Typst.ManifestRequiredFields` | A `[package]` table missing `name`, `version`, `entrypoint`, `authors` or the other required fields |
| `Typst.PackageNameNotCanonical` | A name that collides with a generic or reserved one already common in the registry |
| `Typst.PackageNameKebabCase` | Uppercase or underscores in the package name |
| `Typst.DescriptionStyle` | A description missing its full stop, or opening with `A `/`An ` |
| `Typst.ManifestVersionMatchesDir` | A manifest `version` that disagrees with the `preview/<name>/<version>/` path it sits under |
| `Typst.ReadmeVersionMatchesManifest` | `@preview/<name>:X.Y.Z` examples in the README pinned to a stale version |
| `Typst.RelativeImportInPublishedReadme` | README examples importing by relative path, which works locally and breaks for everyone else |
| `Typst.LicenseFileMatchesSPDX` | A missing LICENSE, or a manifest `license` field that is not a valid SPDX identifier |
| `Typst.LicenseMissing` | No LICENSE file at all — Universe requires one |
| `Typst.BundleIncludesPdf` | Root-level PDFs bloating the published bundle |
| `Typst.ChangelogMissing` | No CHANGELOG.md; not a hard Universe rule, but expected of a package people depend on |

These checks are not in the [check catalog](/checks/), which is generated
from the engine's built-in set and does not yet reach the Typst trait
(CD-303). Until it does, `cofferdam typst --robot` is the authoritative
list.

## Future adapters

SQL, IaC, and GraphQL adapters follow the same shape — a parser feeding the shared `Check` trait and category model. See the [phased roadmap](https://github.com/TAJD/cofferdam/blob/main/MAINTAINERS.md#phased-build).
