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

## Future adapters

SQL, IaC, and GraphQL adapters follow the same shape — a parser feeding the shared `Check` trait and category model. See the [phased roadmap](https://github.com/TAJD/cofferdam/blob/main/MAINTAINERS.md#phased-build).
