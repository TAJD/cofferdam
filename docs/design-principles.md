# Design principles

A deep-dive on the *why* of cofferdam's architecture — the load-bearing decisions, the leverage they create on downstream repositories, and how the same patterns apply to other domains.

This document is a companion to:

- [`README.md`](../README.md) — what cofferdam is, what it does.
- [`CLAUDE.md`](../CLAUDE.md) — the recipe for writing a check.
- [`docs/plugin-sdk-guide.md`](./plugin-sdk-guide.md) — the JS plugin author guide.
- [`docs/invariants.md`](./invariants.md) — the `cofferdam.invariants.toml` spec format.

It is not a tutorial. If you want to get started, read the README and the plugin-SDK guide. If you want to *understand the system* — what makes it generalisable, where its leverage comes from, and what stays the same when you carry the pattern to a new domain — read on.

---

## 1. How cofferdam imposes structure on downstream repos

The tool's name is the metaphor: a cofferdam is a watertight compartment built around a construction site to keep water out while you work. Cofferdam-the-tool builds a watertight compartment around a TypeScript codebase — flagging anything inside that drifts from the architecture you declared.

It does this through four customisation layers, narrowest to widest:

### 1.1 The four leverage layers

| Layer | Where it lives | What it controls | Who edits it |
|---|---|---|---|
| Built-in checks | `crates/cofferdam-checks/` | The opinionated TS-smell taxonomy that ships in the binary | Cofferdam maintainers |
| Per-check options | `cofferdam.toml`, `[checks."Category.Name"]` | Narrow knobs per rule (limits, allow-lists, severity overrides) | Project owner |
| Project-wide spec | `cofferdam.invariants.toml` | Architectural intent: layers, frozen boundaries, public-API entry points, forbid/require import rules | Project owner / architect |
| Plugin SDK | `@cofferdam/check-sdk`, `plugins = [...]` in config | Custom checks the project ships itself | Project owner |

Built-in checks supply the floor — `Warning.TripleEquals`, `Refactor.CyclomaticComplexity`, `Readability.MaxLineLength`, and forty or so others. Per-check options tune them. The plugin SDK lets a project add its own rules in TypeScript.

But the load-bearing layer is the third one — the project-wide spec. Everything important about cofferdam's leverage on downstream repos flows from the choice to put architectural intent into a single declarative file consumed by *many* checks.

### 1.2 Why `cofferdam.invariants.toml` is the centerpiece

Most linters configure each rule independently. ESLint has a hundred config keys; you tune each rule with its own option bag; if two rules both care about your layering scheme, you encode it twice.

Cofferdam takes a different stance: **architecture is one thing, declared once.** A single file at the repo root says what your codebase is supposed to be:

```toml
[layers]
infra  = ["src/infra/**"]
domain = ["src/domain/**"]
app    = ["src/app/**"]

[layers.allow]
domain = ["infra"]
app    = ["domain", "infra"]

[public_api]
exports = ["package.json:exports", "src/index.ts"]

[boundaries]
"src/legacy/**" = { frozen = true, reason = "see ADR-0007" }

[invariants]
"no-direct-db-access" = { forbid_imports = ["src/infra/db"], from_layers = ["app"] }
```

Several different checks read this file:

- `Design.LayerViolation` interprets `[layers]` + `[layers.allow]` to flag forbidden cross-layer imports.
- `Design.OrphanExport` reads `[public_api]` to suppress findings on legitimate entry points.
- `Design.BoundaryFrozen` reads `[boundaries]` to forbid new code in named subtrees.
- `Design.InvariantViolation` interprets the `[invariants]` table as generic forbid/require import rules.

The same data; many enforcement points. That's the leverage.

It also means three audiences read the same file:

- **Humans** — when an architect joins the team and asks "how is this codebase organised?", the answer is one file, not a folklore.
- **Agents** — `cofferdam advise <file>` outputs the rules that apply *before* the agent edits, so an LLM plans against the spec instead of reverse-engineering it from violations.
- **The engine** — checks consume it directly.

The spec is a contract between human intent and machine enforcement. Once you have that, adding a new rule (in Rust or in TS via the plugin SDK) doesn't require asking the user to configure it — it asks the *spec* what the project meant.

### 1.3 What downstream gets in practice

The four layers don't compose into a free-for-all. The system has a few concrete affordances that make it work in real codebases without becoming noisy:

- **Stable IDs.** Every finding carries a dotted `Category.Name` (`crates/cofferdam-core/src/check.rs:62`) used in suppression comments, baseline files, SARIF rule IDs, and config keys. Renames go through a deprecation window. Identity survives rewrites.
- **Suppression directives.** Inline comments suppress a finding by ID. The engine tracks which suppressions actually fire, and `Consistency.UnusedSuppression` flags directives that no longer suppress anything — so the suppression list does not silently rot.
- **Baseline mode.** A project adopts cofferdam by snapshotting its current findings as a baseline; the gate fires only on *new* findings. This makes it possible to tighten an architecture over time without drowning in legacy noise.
- **Priority-sorted output.** Severity is configured (what failure level), priority is computed (how urgent within that level). The report sorts by what to fix first, not what alphabetised first.
- **Robot mode.** A `--robot` JSON flag emits a stable additive schema. Agents and other tools consume the data directly; built-in checks are forbidden from `println!` so the channel stays clean.

These are the day-to-day surfaces a downstream user touches. The architectural depth is deliberately invisible until they want to write or tune a rule.

---

## 2. Fundamental principles

These are the load-bearing design choices. Each is a real decision in the code with a one-line "why this matters." A different choice on any of them would have produced a different tool.

### 2.1 Static metadata, dynamic dispatch

`CheckMeta` (`crates/cofferdam-core/src/check.rs:58-109`) is a `&'static` struct returned from `Check::meta()`. Capabilities — `requires_types`, `consistency`, `observes_findings`, `autofix`, `options` schema, default severity, base priority — are all declared, not inferred.

Why it matters: the engine plans the run before executing it. It knows which checks need a two-pass orchestration, which need the (planned) ts-morph worker pool, which observe the full finding set during finalize. Scheduling is data-driven from declarations. The check trait stays a normal trait object; the engine never has to introspect a check at runtime to know what to do with it.

### 2.2 Three discrete phases with constrained contexts

The `Check` trait has three execution phases (`crates/cofferdam-core/src/check.rs:216-241`):

- `run(file, ctx)` — once per file, isolated.
- `pass2(file, ctx)` — once per file, after every check's `run` has completed for every file. Used by consistency checks that must see the whole project before flagging deviations from the dominant style.
- `finalize(ctx)` — once per run, after all per-file phases have completed. Used by cross-file checks (orphan exports, duplicate detection, layering analysis).

Each phase gets a different context type. `CheckContext` (`check.rs:119`) holds a file and a parsed AST; `FinalizeContext` (`check.rs:179`) holds only the corpus. What a check can touch is lexically bounded — you cannot reach for the AST in `finalize` because the type system hides it.

Why it matters: the constraints make per-phase parallelism safe by construction, and the contexts double as documentation. A reader of a new check trait impl knows what phase they're in by which method they're inside.

### 2.3 Typed, key-addressed shared state

Cross-file checks aggregate evidence in a `CorpusIndex` (`crates/cofferdam-core/src/corpus.rs:58`). Slots are addressed by `CorpusKey<T>` constants (`corpus.rs:24`):

```rust
static MY_SLOT: CorpusKey<Vec<Fingerprint>> = CorpusKey::new("Category.MyCheck.fingerprints");

ctx.corpus.with_slot(&MY_SLOT, |slot: &mut Vec<Fingerprint>| {
    slot.push(/* per-file evidence */);
});
```

Two checks share storage by referencing the same `static`. The string name plus the type parameter give compile-time-checked sharing; a runtime `TypeId` assertion catches name collisions across mismatched types.

Lock layout: outer `RwLock<HashMap>` for slot lookup, per-slot `Mutex` for value access. Two checks targeting *distinct* keys run concurrently; only same-key access serialises (`corpus.rs:177-220`).

Why it matters: this is how cross-file collaboration happens without making the trait a god-object. Most check abstractions either (a) hand the trait a single typed scratchpad and force everything through it, or (b) hand it a `Box<dyn Any>` bag and force every reader to downcast. `CorpusKey<T>` is the third option — typed, scoped, and parallel-friendly.

### 2.4 Capability declaration over capability inference

Closely related to #2.1 but worth calling out separately. A check declares what it needs and what it produces; the engine does not introspect.

- `requires_types = true` ⇒ engine routes the check to a type-aware backend.
- `consistency = true` ⇒ engine runs `pass2`.
- `observes_findings = true` ⇒ engine defers `finalize` until after every other finalize has completed and a `ALL_PRE_FILTER_FINDINGS` snapshot is rebuilt (`check.rs:100-108`).
- `autofix = true` ⇒ `cofferdam fix --dry-run` predicts what will change.
- `options: &[OptionSpec]` ⇒ engine validates user config at startup, surfaces typos before file 4000.

Why it matters: the engine and the check are decoupled. A new capability is added to `CheckMeta` and the engine learns to route it; existing checks that don't opt in keep working. New built-ins or plugins read like Lego — set the flags, get the orchestration.

### 2.5 Stable IDs as identity

Check IDs are dotted (`Category.Name`), stable, and used in: suppression comments (`// cofferdam-disable-next-line Warning.TripleEquals`), baseline files, SARIF rule IDs, config keys (`[checks."Warning.TripleEquals"]`), `cofferdam advise` output, and the docs catalog filename.

The rule: **never rename without a deprecation window.** A user's suppression comments are durable, sometimes years old; the system must not break them silently.

Why it matters: identity is what lets cofferdam survive iterative refactors of its own check set. Renaming a Rust type doesn't reach into a downstream repo's `// cofferdam-disable …` comments because the public name is the dotted ID, not the type name.

### 2.6 Output discipline: one channel

`Issue` (`crates/cofferdam-core/src/issue.rs`) is the only legal "say something" channel from a check. Built-in checks are forbidden from `println!` and `dbg!` (`CLAUDE.md` rule); the JSON output schema is additive-only.

Why it matters: this is what makes the `--robot` mode trustworthy. Every channel a check might use to leak information bypasses suppression, baseline diffing, severity assignment, and the priority computation. Funnelling everything through `Issue` is the discipline that lets agents consume the output as data instead of trying to parse stderr.

### 2.7 Suppression as observed metadata

Suppression is not a special-case in the engine. The two-phase `finalize` (`crates/cofferdam-engine/src/lib.rs`, the cd-wqc fix) lets a check declare `observes_findings = true` and run *after* every other finalize has emitted, with a snapshot of the full pre-filter finding set in the corpus.

Today only `Consistency.UnusedSuppression` uses this. It reads the suppression directives, checks whether each one corresponds to a finding the engine actually emitted, and flags stale ones. The mechanism is real, generic, and reusable; it's just that we have one user.

Why it matters: meta-checks (rules about rules) are part of the same system as ordinary rules. There is no "suppression engine" that lives outside the check pipeline — there is one check that happens to look at every other check's output.

### 2.8 Spec as shared truth

This is principle #1.2 from a fundamentals lens. `cofferdam.invariants.toml` is a versioned, structured, typed artifact that humans, agents, and multiple checks consume.

Why it matters: it's the right level of DRY. Per-rule config keeps repeating "what is this codebase?"; one spec captures the answer once and lets every interpreter ask the spec. This is the most generalisable property of cofferdam — the rest of the architecture is good plumbing, but *spec-as-shared-truth* is the idea that travels.

---

## 3. Generalising to other domains

The transferable pattern, in one sentence: **a typed parsed corpus over a project, plus capability-tagged rules with declarative metadata, plus a single domain spec that many rules interpret, plus stable rule IDs.** Substitute the parser; keep the rest.

### 3.1 What you swap

| Component | TypeScript (cofferdam today) | Other domain |
|---|---|---|
| Parser | `oxc_parser` | tree-sitter, language-specific parser, schema parser |
| Per-file unit | `SourceFile` + `ParsedView` | language file, schema doc, manifest |
| AST visitor | `oxc_ast_visit::Visit` | tree-sitter Query, GraphQL visitor, jsonpath |
| Domain taxonomy | five categories (Warning, Refactor, Design, Readability, Consistency) | domain-appropriate buckets |
| Spec format | `cofferdam.invariants.toml` schema | a different TOML/YAML/JSON schema |

### 3.2 What you keep

- The `Check` trait shape: `meta()` + `run()` + optional `pass2()` + optional `finalize()` + optional `autofix()`.
- `CheckMeta` with capability flags (`requires_types`, `consistency`, `observes_findings`, `options`, etc.).
- `CorpusKey<T>` — typed, key-addressed, per-slot mutex.
- Phase orchestration: per-file, then per-file pass-2 (consistency), then finalize.
- Stable dotted IDs as identity.
- Output discipline: one `Issue` channel; additive schema.
- A single domain spec consumed by many rules.

### 3.3 Domain examples

**Other programming languages.** Swap `oxc_parser` for tree-sitter, keep everything else. The five-category taxonomy is well-tuned to "code style + smells" and likely transfers; if a language has a strong existing dialect (Elixir's Credo, Ruby's Rubocop, Go's vet) borrow their categories. Cross-file checks are where the design pays off — Go's `go vet` does file-level work; the corpus pattern would let a Go-Cofferdam enforce import-cycle invariants the way `Design.LayerViolation` does today.

**Schema and IDL languages** (GraphQL, protobuf, OpenAPI, JSON Schema). The "files" are schemas, the "AST" is the schema tree. `invariants.toml` becomes "API guidelines" — naming conventions, required fields, deprecation rules, breaking-change detection. The corpus collects field types and names across schema files; finalize emits "field renamed without alias" or "required field added without server bump." This is where stable IDs shine — a schema-rule ID outliving a schema rewrite is precious.

**Infrastructure as code** (Terraform, Kubernetes manifests, Helm charts). Files are HCL/YAML manifests; "layers" become *environments* (`prod`, `staging`, `dev`) or *blast-radius rings*; the spec declares which environments are allowed to invoke which modules; cross-file invariants enforce things like "no public IP from a `prod-*` module" or "every IAM role used in prod must originate from a vetted module." `requires_types`-style capability flags can route to a Terraform plan parser instead of static text.

**SQL migrations and database schemas.** Each migration is a file; the corpus accumulates per-table column types across the migration history; `finalize` emits "column type changed without backward-compatible shim" or "index dropped from a hot table without an explanation." The two-phase consistency mode fits naturally — pass 1 learns the canonical column-naming convention, pass 2 flags deviations.

**Configuration sprawl.** A modern JS project has `package.json`, `tsconfig.json`, `.eslintrc`, `.prettierrc`, `cofferdam.toml`, possibly `turbo.json` and a Vite config. Cross-file invariants matter: "if `tsconfig.strict` is on, `eslint:no-explicit-any` must be `error`"; "every workspace package's `exports` must include both `import` and `require` keys." The corpus pattern treats each config as a parsed document and finalize emits cross-config inconsistencies.

**Documentation and runbooks.** Files are markdown; cross-file checks catch broken intra-doc links, undocumented public symbols (joined against the code corpus), drift between `README.md`'s installation steps and the `package.json` `engines` field. The spec declares which directories are public docs vs internal scratch.

**Process artifacts** — RFC repos, ADR collections, even policy documents. The engine doesn't care what the files mean; it cares about parsing them and applying rules. A team that maintains a discipline (every ADR cites at least one stakeholder; every RFC has a status; every retired ADR has a successor link) can encode that in a check.

### 3.4 What does *not* transfer

- The five-category taxonomy is tuned to TS code-quality. A schema validator might want three (Naming, Compatibility, Conventions). The taxonomy is configurable in principle (see the closed-enum bead in this epic); ship per-domain.
- The autofix engine assumes byte-offset edits in the original source. Domains with semantic diffs (schema migrations, IaC plans) need a different fix model.
- Severity and priority semantics are project-specific. A schema validator's "Breaking" severity is more like cofferdam's `Warning.severity = "error"` than its `Warning` category.

---

## 4. Reading guide for new contributors

If you're new to the codebase and want to internalise the design:

1. Read `crates/cofferdam-core/src/check.rs` start-to-finish — the central trait, contexts, and metadata.
2. Read `crates/cofferdam-core/src/corpus.rs` — the cross-file shared-state mechanism. Look at the test cases; they document the locking model.
3. Read one self-contained check: `crates/cofferdam-checks/src/warning.rs::TripleEquals` (AST visitor, single file) and one cross-file check: `crates/cofferdam-checks/src/design.rs::DuplicateExportName` (corpus producer + finalize emitter).
4. Read `crates/cofferdam-core/src/invariants.rs` — the spec parser and round-trip contract.
5. Read `docs/invariants.md` for the user-facing spec semantics.

After that, the engine's orchestration loop in `crates/cofferdam-engine/src/lib.rs` reads as expected — it's the data-driven scheduler implied by the principles above.

---

## Glossary

Terms used in this document and across the codebase, sorted alphabetically. Code anchors point to definitions.

**AST view (`AstView`)** — the stable, plugin-facing surface for walking a parsed file. Built-in Rust checks may use `oxc_ast_visit` directly; plugins consume `AstView` for ABI stability across cofferdam versions. `crates/cofferdam-core/src/ast.rs`.

**Autofix** — a check's optional `autofix(&Issue, &SourceFile) -> Option<TextEdit>` method. Returning `Some(edit)` opts the check into mechanical fixing via `cofferdam fix`. The fix engine processes edits in reverse byte-offset order to avoid span-shift bugs. `crates/cofferdam-core/src/check.rs:238`.

**Baseline** — a snapshot of the project's findings at a point in time, used by `cofferdam check` to suppress pre-existing findings and gate only on new ones. Adopted via `cofferdam init --baseline`. The mechanism that makes legacy adoption possible without drowning in noise.

**Base priority** — a check's declared priority floor in the range -20..=20 (`CheckMeta::base_priority`, `crates/cofferdam-core/src/check.rs:66`). Per-issue priority is computed from this plus signal-specific contributions (severity override, file size, etc.). Priority is computed; severity is configured.

**Boundary (frozen)** — a glob pattern marked `frozen = true` in `[boundaries]` of `cofferdam.invariants.toml`. New code added under the pattern triggers `Design.BoundaryFrozen`. Used to retire legacy areas of the codebase incrementally.

**Category** — one of five buckets a check belongs to: Consistency, Design, Readability, Refactor, Warning. Currently a closed Rust enum (`crates/cofferdam-core/src/check.rs:29`); doc claims projects can add categories — see bead `cd-9hp.3` for the open question.

**Check** — a unit of analysis. A Rust type implementing the `Check` trait, or a JS plugin produced by `defineCheck`. Has static metadata (`CheckMeta`) and three execution methods (`run`, `pass2`, `finalize`) plus optional `autofix`. `crates/cofferdam-core/src/check.rs:216`.

**`CheckContext` / `FinalizeContext`** — the contexts handed to a check during per-file and cross-file phases respectively. Each lexically constrains what the check can touch (file, AST, options, corpus). `crates/cofferdam-core/src/check.rs:119` and `:179`.

**`CheckMeta`** — declarative static metadata about a check: ID, category, base priority, default severity, capability flags, options schema. Returned as `&'static` from `Check::meta()`. The single source of truth the engine reads to schedule a run. `crates/cofferdam-core/src/check.rs:58`.

**Consistency mode** — checks declaring `consistency = true` in their meta opt into a two-pass orchestration: pass 1 collects per-file evidence (e.g. quote-style frequency); pass 2 flags deviations from the dominant pattern. Implemented via the `pass2` method on `Check`. `crates/cofferdam-core/src/check.rs:226`.

**Corpus / `CorpusIndex`** — the run-scoped shared store for cross-file checks. One instance per analysis run; passed into every `CheckContext` and the eventual `FinalizeContext`. Slots are addressed by `CorpusKey<T>`; the outer map is `RwLock<HashMap>`, each slot has its own `Mutex`. `crates/cofferdam-core/src/corpus.rs:58`.

**`CorpusKey<T>`** — a typed handle to a corpus slot. The `&'static str` name is the storage key; the `T` parameter ties it to one Rust type at compile time. Two checks sharing the same `CorpusKey` constant share storage. Mismatched `T`s on the same name panic at runtime — acceptable for built-ins, hostile to plugins (see bead `cd-9hp.7`). `crates/cofferdam-core/src/corpus.rs:24`.

**Decision record / ADR** — a short markdown file capturing an architectural choice and its reasoning. Cofferdam's `[boundaries]` entries can reference ADRs in their `reason` field; a project-driven convention.

**Default severity** — a check's declared severity in `CheckMeta::default_severity`, used for every emitted `Issue` unless the project overrides via `[checks."X.Y"] severity = "..."`. Severity is an axis separate from priority. `crates/cofferdam-core/src/check.rs:72`.

**`finalize`** — the cross-file phase. One call per check per run, after every file has been through `run` (and `pass2` for consistency checks). Sees the corpus; emits `Issue`s the same way `run` does. Used by project-graph rules (orphan exports, duplicate detection, layer audits). `crates/cofferdam-core/src/check.rs:229`.

**Finding** — synonym for `Issue` in user-facing prose. The unit of output a check emits.

**`Issue`** — the only legal output of a check. Carries span, message, severity, ID, optional `related: Vec<RelatedSpan>` for findings that point to multiple locations, optional autofix hint. `crates/cofferdam-core/src/issue.rs`.

**Invariants spec** — the contents of `cofferdam.invariants.toml`. A declarative architectural contract: layers, layer-allow rules, public API entry points, frozen boundaries, generic forbid/require import rules. Multiple checks read the same spec. `crates/cofferdam-core/src/invariants.rs`.

**Layer** — a named glob pattern set in `[layers]` of the invariants spec. Files are resolved to a layer by longest-non-glob-prefix specificity (`crates/cofferdam-core/src/layers.rs:42`). `[layers.allow]` declares which layers may import from which.

**Observer check** — a check with `observes_findings = true` in its meta. Its `finalize` is deferred until every other check's `finalize` has emitted, then it runs with a snapshot of the full pre-filter finding set in the corpus. Today only `Consistency.UnusedSuppression`. `crates/cofferdam-core/src/check.rs:100`.

**Options schema (`OptionSpec`)** — a check's declared list of configurable knobs. Validated against user `cofferdam.toml` at engine startup; resolved values are lent to the running check via `CheckContext::options`. `crates/cofferdam-core/src/options.rs`.

**`pass2`** — the per-file second-pass method on `Check`. Called for consistency checks after every check's `run` has completed for every file. Reads pass-1 evidence from the corpus; emits findings for files that deviate from the discovered convention. `crates/cofferdam-core/src/check.rs:226`.

**Plugin** — a TypeScript module exporting a `defineCheck`-produced object. Loaded via `plugins = [...]` in `cofferdam.toml`. Runs alongside built-in checks; findings flow through the same priority computation, suppression, baseline, and output paths. See `docs/plugin-sdk-guide.md`.

**Priority** — the computed urgency of a finding within its severity. Sorted descending in output; this is how cofferdam answers "what should I fix first?". Distinct from severity.

**Public API** — entry-point sentinels declared in `[public_api]` of the invariants spec. `Design.OrphanExport` skips findings on these files because they are *supposed* to be exported. Resolved at spec-load time. `crates/cofferdam-core/src/invariants.rs:280`.

**Robot mode** — the `--robot` JSON output flag. Stable additive schema; the contract between cofferdam and machine consumers (CI, agents). Built-in checks are forbidden from writing to stdout/stderr to keep this channel clean.

**`run`** — the per-file phase of a check. Called once per file per run; returns the findings for that file. The default phase; most checks use only this one. `crates/cofferdam-core/src/check.rs:218`.

**SARIF** — Static Analysis Results Interchange Format. One of cofferdam's output formats; rule IDs are the dotted check IDs. The contract is JSON schema; additive changes only.

**Severity** — a finding's failure level (error / warning / info). Configured per check (and overridable in `cofferdam.toml`). Distinct from priority. Severity gates CI; priority orders the report.

**`SourceFile`** — the engine's representation of a single file: text, path, optional resolved layer. Passed by reference into `CheckContext`. `crates/cofferdam-core/src/source.rs`.

**`Span`** — a byte-offset range plus line/column, used to point at a finding's location in the source. Construct via `cofferdam_core::span_from_bytes(text, start, end)`; rolling your own loses a UTF-8 column-counting nuance. `crates/cofferdam-core/src/span_util.rs`.

**Suppression directive** — an inline comment that suppresses a finding by check ID:

```ts
// cofferdam-disable-next-line Warning.TripleEquals
if (a == b) { /* legitimate ergonomic == */ }
```

Tracked by the engine; `Consistency.UnusedSuppression` flags directives that don't actually suppress anything (stale).

**Two-phase finalize** — the engine's finalize-stage orchestration. Phase A runs every check whose `observes_findings` is `false`; the engine then rebuilds the `ALL_PRE_FILTER_FINDINGS` corpus snapshot from run + pass2 + Phase A; Phase B runs the observers. Documented in `CLAUDE.md` under "Engine finalize ordering."

---

*Last revised 2026-05-06. The architecture-extension epic that frames the open work is `cd-9hp`; its top-lever child bead is `cd-9hp.1` (embedded predicate DSL inside `cofferdam.invariants.toml`).*
