# cd-r62 + cd-47m — baseline tooling

Status: **approved 2026-05-06**, ready to implement as one paired PR.

## Why paired

Both edits land in `cofferdam-cli/src/main.rs::run_baseline_write` and
`cofferdam-engine/src/baseline.rs`. Pairing avoids merge churn between
two agents on the same function.

## cd-r62 — baseline ↔ suppression hygiene

**Decision: ship the non-breaking variant first, defer active pruning.**

The bead's option 2 (lint-only). Don't actively prune on `baseline
write`. Surface the audit trap loudly so teams can react in the next
release cycle. (Active pruning is a behaviour change for everyone
running `baseline write` and warrants its own release note — file as
`cd-r62b` once teams have had ≥1 release of the lint output.)

### `cofferdam baseline lint` subcommand

Walks the files referenced in `.cofferdam/baseline.json`, parses
suppressions per file (reuse `cofferdam-engine::suppress::Suppressions::parse`),
and reports any baseline entry whose `(file, line, check_id)` is now
also covered by an inline directive.

```text
$ cofferdam baseline lint
~ lib/y.ts:32 — Warning.UnusedImport is in the baseline AND suppressed inline
    (cofferdam-ignore: Warning.UnusedImport: legacy)
~ src/foo.ts:81 — Refactor.CyclomaticComplexity is in the baseline AND
    suppressed by `cofferdam-ignore-file: Refactor.CyclomaticComplexity`
2 dual-state entr(ies) found
```

Exit codes: `0` if none found, `1` if any (so it's wireable into CI for
teams that want to enforce). `--robot` emits stable JSON:

```json
{
  "dual_state": [
    {
      "file": "lib/y.ts",
      "line": 32,
      "check_id": "Warning.UnusedImport",
      "suppression_form": "next_line",
      "suppression_text": "// cofferdam-ignore: Warning.UnusedImport: legacy"
    }
  ],
  "summary": { "count": 1 }
}
```

`suppression_form` values: `next_line` | `range` | `file`.

### Doctor integration

New `baseline-suppression` check in `crates/cofferdam-cli/src/doctor.rs`.
Runs only when both a baseline file and source files exist; passes when
no overlap, warns when overlap found:

```text
~ baseline-suppression  3 baseline entr(ies) also have inline suppressions
                         → run `cofferdam baseline lint` for the full list
```

Doctor stays exit-0 on warns (matches cd-5lh's pattern).

### What this does NOT do

- Active pruning on `baseline write` — explicitly deferred.
- Auto-fix mode (`baseline lint --fix` to remove dual-state entries) —
  same follow-up.
- Reverse: detect inline suppressions whose underlying check no longer
  fires (that's `Consistency.UnusedSuppression`, cd-pph, already shipped).

## cd-47m — `baseline write` reports delta vs prior

Same `run_baseline_write` function, additive. Before overwriting, read
the existing `.cofferdam/baseline.json` if present, compute the delta,
and emit a one-or-two-line summary after the existing
`wrote N finding(s)` line.

### Output

```text
$ cofferdam baseline write src/
✓ wrote .cofferdam/baseline.json (581 finding(s))
  delta vs prior: -254 (added 0, removed 254)
  by check:
    Warning.UnusedImport            -76
    Warning.NoConsoleLog            -60
    Readability.MaxLineLength       -70
    Readability.MaxFunctionLength   -48
```

Edge cases:

- **First run (no prior baseline):** skip the delta section entirely;
  keep the existing one-line output.
- **Delta is exactly zero:** still print `delta vs prior: 0` so users
  know the read-and-compare path ran.
- **`--robot` mode:** add a `delta: { added: N, removed: M, by_check:
  { ... } }` block to the existing JSON shape. Omitted when no prior
  baseline.

### "By check" listing rules

Cap at the top 10 changes (sorted by absolute magnitude); ties break
alphabetically. A `... and K more` line covers the tail. Avoids dumping
a 50-row table in a CI log.

### Bonus subcommand `cofferdam baseline diff [<path>]`

Cheap once the comparison logic exists. Extract the delta computer into
`baseline::compute_delta(prior: &Baseline, current: &Baseline) -> Delta`
and have both the `write` summary and the new `Cmd::Baseline::Diff
{ path: Option<PathBuf>, robot, pretty }` use it.

`Cmd::Baseline::Diff` reads two baselines (or one + scans the working
tree) and prints the same delta block standalone. Default behaviour
(`cofferdam baseline diff` no args): compare `.cofferdam/baseline.json`
against the current findings (run the engine, compute the delta, exit
without writing). With one path arg: compare two baseline JSON files.

Useful for triage outside the rewrite path.

## Comparison key

What counts as "same finding" across baselines? Use the existing
baseline `signature` field (stable across line shifts), falling back to
`(file, check_id, line)` exact match. Mirror however baseline matching
already works in the engine — don't invent a new comparison key.

## Files to touch

- `crates/cofferdam-engine/src/baseline.rs`
  - Add `Delta { added: usize, removed: usize, by_check: BTreeMap<String, i64> }`.
  - Add `compute_delta(prior: &Baseline, current: &Baseline) -> Delta`.
  - Tests for added / removed / changed / identical baselines.
- `crates/cofferdam-cli/src/main.rs`
  - New `BaselineAction::Lint { robot, pretty }` variant + dispatch.
  - New `BaselineAction::Diff { path: Option<PathBuf>, robot, pretty }` + dispatch.
  - Update `run_baseline_write` to read prior, call `compute_delta`,
    render summary; thread `--robot` through.
- `crates/cofferdam-cli/src/baseline_lint.rs` (new module) —
  `Cmd::Baseline::Lint` implementation.
- `crates/cofferdam-cli/src/baseline_diff.rs` (new module) —
  `Cmd::Baseline::Diff` implementation.
- `crates/cofferdam-cli/src/doctor.rs` — new `check_baseline_suppression`.
- `crates/cofferdam-cli/tests/` — integration tests:
  - tempdir with a baselined finding + an inline `cofferdam-ignore: <CheckId>`
    covering the same line; assert lint reports it; remove the directive
    and assert lint is silent.
  - tempdir with two baseline files; assert `baseline diff <a> <b>`
    matches `compute_delta` over the same inputs.

## Out of scope

- Active pruning on `baseline write` (cd-r62b follow-up).
- Auto-fix mode for `baseline lint`.
- Refactoring the existing `run_baseline_write` beyond the delta-summary
  addition. If the function is already a mess, the agent should NOT
  rewrite it as part of this change.

## Risks / what was considered

- **Performance.** `compute_delta` is O(N + M) over the two finding
  lists keyed by signature. baseline.json on bestefforttools is ~400
  entries; nothing to worry about until 100k.
- **Doctor exit semantics.** The new `baseline-suppression` warn
  doesn't change doctor's exit-0-on-warn contract.
- **Rejected: ship active pruning now.** Behaviour change for everyone
  using `baseline write`, even if conceptually right. One release of
  lint surface first; teams react; then ship pruning with a clear
  release note.
