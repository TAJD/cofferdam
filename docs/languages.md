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

**Caveat — template source directories (CD-101):** tree-sitter-html has no
grammar for template-engine delimiters. Scanning *unrendered* template
source (Flask `templates/`, Rails `app/views/`, Express `views/`) with the
default `.html`/`.htm` extension globs routinely trips `Warning.ParseError`
on legitimate idioms — an ERB/EJS `<% %>` scriptlet, or a Jinja
`&#123;&#123; url_for("x") &#125;&#125;` nested inside a double-quoted attribute — that are
only invalid as raw HTML, not bugs. `Warning.ParseError` from the HTML
adapter's recovered-parse path is `Low` severity for exactly this reason,
so it won't fail CI at the default `--fail-on=medium`. If you'd rather not
see it at all, exclude the template directory (`[[overrides]] disabled =
true` scoped to that path, or a `.cofferdamignore` entry) until it's
rendered/compiled — [`cofferdam verify --dist`](/verify-dist) already
covers the rendered-output case correctly.

## Rust (second language, dogfood)

A Rust adapter ships under `crates/cofferdam-rust`, exercising the engine's per-language dispatch (cd-91zc). Three checks today:

- `Rust.NoUnwrapInLib` — no `.unwrap()` in library code
- `Rust.NoUnimplementedInNonTest` — no `unimplemented!()` outside tests
- `Rust.MissingPubDoc` — public items need doc comments

A CI dogfood job runs cofferdam against its own Rust workspace on every PR.

The Rust adapter is load-bearing for the polylingual architecture pledge: until a second domain existed, "framework, not TS-tool" was theory. It now ships as a working second parser feeding the same `Check` trait the TS adapter uses.

## Future adapters

SQL, IaC, and GraphQL adapters follow the same shape — a parser feeding the shared `Check` trait and category model. See the [phased roadmap](https://github.com/TAJD/cofferdam/blob/main/MAINTAINERS.md#phased-build).
