# Roadmap

Cofferdam's detailed backlog lives in a private tracker. This file is the public
mirror: what we're building, in what order, and what we've decided not to build.
Ticket references (`CD-nnn`) appear for provenance — they aren't public links.

Last synced: **2026-08-12**.

---

## Where this is going: 0.5 → 1.0

**Cofferdam is a policy engine for codebases.** You declare what your codebase
must be — layers, boundaries, import rules, arbitrary predicates — in one spec
file (`cofferdam.invariants.toml`). Cofferdam evaluates those policies over a
graph of the project and delivers the verdict three ways:

1. **Before an edit** — `advise`, a static projection of policy onto a path,
   consumed by agents via hooks and MCP.
2. **At commit** — `advise --diff`, a `would_fire` / `would_clear` set-diff on
   span signatures.
3. **In CI** — `check`, with the `[budgets]` ratchet that only tightens.

In one line: *declared architecture, mechanically enforced, delivered to agents
before they write and to CI after.*

The dimension we intend to be best at is **expressiveness of the policy language
× quality of enforcement delivery** — not check count, not language coverage,
not code intelligence.

## What Cofferdam is not

Each of these implies deletions, most of them landing in 0.5.

1. **Not a linter.** No opinion on style or smells. A check ships only if it
   reads the spec, needs the corpus/graph, or needs the pass-2 consistency mode.
   Biome and oxlint own the rest — and 0.5 ships a table pointing every removed
   check at its replacement.
2. **Not a code-intelligence server.** Blast radius, symbol search and impact
   analysis are a fast-moving field we're not racing. Cofferdam models *declared
   intent*, not derived structure. `context` survives as the projection of
   policy and rationale onto a diff.
3. **Not a metrics product.** The only numbers we keep are counts against
   budgets, because those gate.
4. **Not polylingual yet.** TypeScript only, until one unchanged rule can
   enforce the same policy across two languages on the promoted graph.
5. **Not an autofix engine.** The agent is the fix engine. Our job is a precise
   verdict with rationale, so `fix` goes.
6. **Not a knowledge base.** Notes survive as *rationale attached to policies*.
   Free-floating tribal knowledge belongs in AGENTS.md.

## The policy promotion ladder

Every convention starts at the cheapest rung and gets promoted when it earns it:

```
knowledge note (prose, advisory)
  → plugin (fuzzy heuristics, project-specific judgment — the SDK's permanent home)
    → spec policy (crisp enough to be a graph query — enforced, advisable, ratchetable)
```

Making each promotion cheap is a design goal. Policies that sit at the plugin
rung across several repos *are* the DSL backlog.

Some policies correctly stay plugins forever. Our pilot repo has a colour-context
heuristic — a bare `#add` only counts as a colour when the surrounding line
smells like one — and that judgment is too fuzzy to be a graph query. It stays a
plugin, deliberately, and is documented as the canonical example of where the
boundary sits.

**One rule holds throughout:** any policy you can't express is a missing graph
schema element or a missing operator — never a text-regex escape hatch. There
will be no `file contains /pattern/` operator. Domain facts get extracted by
adapters into the graph; rules stay pure graph queries.

---

## Workstreams

| | Workstream | Status | Gist |
|---|---|---|---|
| **A** | Subtraction (0.5) | Next | Delete linter-level checks, the type host, side quests, `fix` |
| **B** | Delivery fixes | In progress | Plugins become first-class; plugin metadata reaches `advise` and MCP |
| **C** | Graph promotion | Planned | Corpus slots become an indexed graph store, zero behaviour change |
| **D** | DSL v1.x | Planned | New operators, driven strictly by a documented real-world policy corpus |
| **E** | UI facts + token discipline | Planned | Adapters extract design-system facts into the graph |
| **F** | Pilot migration | Planned | ~250 lines of plugin become ~30 lines of spec, at finding parity |
| **G** | Rationale + verdict format | Planned | Every finding says why the rule exists and what would satisfy it |
| **H** | Docs & README | Next | The repositioning, written down |

### A — Subtraction (release 0.5)

Mostly deletion, shipped as one release with a migration table.

- Cut roughly 20 linter-level checks; the catalog lands near 25, every survivor
  passing the stay/cut test.
- Retire the ts-morph type host — its only consumer is one of the cut checks.
  The result is a single static binary with **no Node on the default path**; the
  plugin host stays the one opt-in Node surface. The `TypeOracle` trait survives
  dormant for future design-level typed checks.
- Evict side quests: the typst crate, the HTML adapter and `verify --dist`, and
  the Rust adapter (whose three clippy-equivalent checks are exactly the
  non-goal-1 failure). The Rust adapter returns later as the second language in
  the shared-rule test, once the graph and DSL have settled.
- Remove `fix`.
- Publish the migration table: every removed check → its Biome, oxlint, Knip or
  dependency-cruiser equivalent, or an honest "intentionally dropped".

### B — Delivery fixes

Four of the five originally-scoped bugs have shipped (`CD-321`, `CD-70`,
`CD-78`, `CD-319`). What remains:

- **Plugin `CheckMeta` projects through `advise` and MCP** — an agent about to
  edit a file matched by a plugin's `pathPatterns` sees that plugin's
  constraints *before* it writes. This converts the pilot's design system from
  post-hoc enforcement into pre-edit guidance with zero DSL work, and it is the
  highest leverage-to-effort item on the roadmap. The CLI/MCP byte-parity
  guarantee holds.
- **One glob implementation** shared by spec selectors, config overrides and
  plugin `pathPatterns`. `CD-70` fixed a symptom; the cause is that path
  matching is implemented more than once, and every drift between them fails
  silently.

### C — Graph promotion

The `IMPORTS` / `EXPORTS` corpus slots become a real indexed graph store —
nodes for file, symbol, export and layer; typed edges for `imports`,
`imports-type-only`, `exports`, `declares`, `member-of`, `calls`; fast
transitive reachability.

The invariant that matters: **everything queryable, nothing derivable only
inside a hand-written `finalize`**. Today a rule can only ask what some check
already computed, and that is the ceiling on how expressive the policy language
can get.

> **Gate.** The three existing spec checks (`Design.LayerViolation`,
> `Design.InvariantViolation`, `Design.ScriptedInvariant`) get reimplemented as
> pure graph queries with the snapshot suite **unchanged** — not "equivalent
> modulo ordering", unchanged. No DSL work merges until that passes.

### D — DSL v1.x

**No operator without a named policy in a real repo that needs it, migrating in
the same release.** That rule is what keeps the DSL from becoming a
general-purpose query language nobody can learn.

> **Gate.** A documented policy corpus comes first — about a dozen real policies
> from the pilot repo and from Cofferdam's own contributor invariants, each
> marked *expressible today* / *needs operator X* / *stays a plugin*, with a
> column for what dependency-cruiser, ESLint and ArchUnitTS can express. No
> operator work starts before that document exists.

Operators in view, each tied to a named policy:

- `declares` — "no island may declare its own `buildHeaders`".
- `calls` — "no island may call `fetch` directly, excluding ui-primitives".
- Conditional require over extracted facts — "a `btn` class present implies the
  file imports the Button primitive".
- Quantifiers and negation over transitive closure — "nothing in `domain`
  reaches `infra` at any depth" — but only as the corpus demands, and always
  reporting the witness path.

Cofferdam's own CI will enforce its own contributor invariants through its own
spec as each becomes expressible. The product claim, demonstrated rather than
asserted.

### E — UI facts and token discipline

Adapters extract design-system facts into the graph: JSX class values, CSS class
definitions, style values, and token definitions from declared token sources.
A `[design_tokens]` spec section names those sources, so token values become
*derived* rather than copied — today our pilot's plugin hardcodes `4px` and
`0.375rem` transcribed by hand from a layout file, and nobody finds out when
they drift.

That unlocks three generic policies: no raw colour literal outside token
sources; no inline style value shadowing a declared token; no primitive-shaped
CSS class defined outside ui-primitives.

**Scope guard:** we extract only facts a declared policy queries. This is not a
CSS analyzer — Stylelint exists.

### F — Pilot migration

Our pilot adopter's two plugins are the migration test corpus. Policies move to
the spec; spec and plugin run side by side; findings are diffed against the
plugins' recorded expectations until they agree. **Parity blocks deletion** —
nothing gets removed because the spec "looks equivalent".

What's left afterwards is a small plugin holding the colour-context heuristic
and nothing else. That residue is the designed outcome, not an unfinished
migration.

### G — Rationale and verdict format

Notes get stable IDs, and policies link to them two ways: `rationale` on the
spec entry, `enforces` in the note's frontmatter. Two lints follow — an
invariant with no rationale warns (a coverage gap you'll have plenty of on day
one), a note claiming to enforce a rule that doesn't exist errors (it's lying
about the codebase).

Then every finding and every advise item answers four questions:

1. What fired.
2. Why the policy exists — rationale inline.
3. What would satisfy it — the constraint restated positively.
4. What an override costs — the suppression syntax, and who reviews it.

Question 3 is what replaces `fix`. Dropping autofix only pays if the verdict is
good enough to act on.

The wider knowledge-layer roadmap — staleness engine, change-shape selectors,
spec-coverage ratchet — is deliberately deferred until an adopter asks for it.

### H — Docs

The README gets rewritten around the definition above, with the hierarchy
inverted: `advise` and `context` are the product, `check` is the CI backend. The
non-goals ship verbatim. Three architecture decision records land — the
repositioning and the stay/cut test, the promotion ladder, and the no-regex
discipline. Plus the migration guide, the policy corpus, and this file.

`gen-docs --check` runs in the pre-commit hook throughout, so the docs can't
drift from the binary.

---

## Sequencing

```
A (subtraction) ──┐
B (delivery fixes)┴─→ 0.5 release + docs
        │
        v
D0 (policy corpus doc, no code)
        │
        v
C (graph promotion, gated) ───→ declares, calls ──→ pilot policies 5–6
        │                                                   │
        v                                                   v
E (UI facts + tokens) ────────→ conditional require ──→ pilot policies 1–4
        │
        v
G (rationale + verdict format) → before/after writeup → launch
```

Three hard gates: the zero-behaviour-change graph test blocks all DSL work; the
policy corpus document blocks every new operator; finding parity blocks deleting
any plugin code in the pilot migration.

## Explicitly not in this arc

- Knowledge-layer staleness engine, change-shape selectors, spec-coverage
  ratchet — deferred until an adopter pulls for them.
- A resident daemon / warm-process architecture. Sequenced *after* this work: a
  fast engine on a weak policy language is the wrong order.
- A second language adapter and the full shared-rule test. The pilot's
  fixture-parity test is its little sibling; the real one waits for the graph
  and DSL to settle.
- `init --infer` adoption tooling.
- Any embeddings or LLM ranking in context relevance. Determinism is the
  product promise.

## How we'll know it worked

1. Catalog at 25 checks or fewer, every one passing the stay/cut test; a single
   static binary with no Node on the default path.
2. The pilot repo's design system enforced from `cofferdam.invariants.toml` plus
   one small plugin, at finding parity with the plugins it replaced.
3. `advise` on a pilot file lists design-system constraints with rationale,
   pre-edit, identically via CLI and MCP.
4. Cofferdam's own CI enforcing at least three of its own contributor invariants
   through its own spec.
5. README, this roadmap, the migration guide, three ADRs and the policy corpus
   published, with `gen-docs --check` green throughout.
