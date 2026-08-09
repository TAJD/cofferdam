# Suppression directives

Sometimes a finding is correct but not actionable right now: a third-party type mismatch you can't fix, a complexity score on generated code you don't own, a `console.log` left deliberately in a debugging harness. Suppression directives let you tell the engine "I see this finding; I'm choosing not to fix it" — inline, in the source file, with an optional reason field so the decision is auditable.

The design goal is **explicit, reasoned, audit-friendly suppression**: every suppression is a line of source code that a reviewer can see, question, and remove. There are no out-of-band suppression lists, no opaque config keys, and no suppression-of-suppressions.

---

## Canonical syntax

Four primary forms. All are single-line comments beginning with `// cofferdam-`.

### Next-line suppression (default)

```ts
// cofferdam-ignore: Warning.NoEval: codegen bootstrap, not user input
eval(generatedCode);
```

Suppresses the finding on **the immediately following line**. The reason field (everything after the second `:`) is optional by default, required for the `Warning` category (see [Reason field](#reason-field)).

The directive must be on its own line, immediately preceding the line with the finding. A directive that has no finding on the next line is a no-op today; a future "unused suppression" check (separate bead) will flag it.

### Range suppression

```ts
// cofferdam-ignore-start: Refactor.CyclomaticComplexity
function generatedRouter(req, res) {
  // ... 200 lines of generated switch logic ...
}
// cofferdam-ignore-end
```

Suppresses the named check for every line between `cofferdam-ignore-start` and the next `cofferdam-ignore-end`. The end marker takes no check ID — a single `cofferdam-ignore-end` closes the most recently opened range. Nested ranges for the same check ID are not supported; use a single wider range instead.

`cofferdam-ignore-end` with no matching start is a no-op.

### File-wide suppression

```ts
// cofferdam-ignore-file: Readability.MaxLineLength
```

Suppresses the named check for the **entire file**. The directive must appear at the top of the file (before any non-comment, non-blank content). A `cofferdam-ignore-file` that appears mid-file is silently ignored by the engine; a future lint-the-suppressions check will flag it.

### ID-less (broad) suppression

All three forms accept an omitted check ID to suppress every check on that scope:

```ts
// cofferdam-ignore
doSomethingWeird();

// cofferdam-ignore-start
// ... block where nothing is checked ...
// cofferdam-ignore-end

// cofferdam-ignore-file   ← at top of file
```

When the engine encounters a broad suppression it still suppresses the findings, but it also emits an info-level diagnostic: `"broad suppression at line N — no check ID specified"`. See [ID-less suppression](#id-less-suppression) for the rationale.

---

## ESLint-style aliases

For teams migrating from ESLint, cofferdam also accepts the following aliases. They are normalised to the canonical form by `cofferdam fix` (and, in a future release, by the formatter). The alias and canonical form are functionally identical — choose whichever your muscle memory prefers.

| Alias | Canonical equivalent |
|---|---|
| `// cofferdam-disable-next-line <CheckId>` | `// cofferdam-ignore: <CheckId>` |
| `// cofferdam-disable-line <CheckId>` | `// cofferdam-ignore: <CheckId>` (applied to the current line) |
| `// cofferdam-disable-block <CheckId>` | `// cofferdam-ignore-start: <CheckId>` |
| `// cofferdam-enable-block` | `// cofferdam-ignore-end` |
| `// cofferdam-disable-file` | `// cofferdam-ignore-file: <CheckId>` (ID-less when no ID given) |

**Why both?** Biome did the suppression design after a decade of watching ESLint accrete confusion — `line` vs `next-line`, block comments vs line comments, no reason field, no audit trail. The canonical form is tighter. But ESLint syntax is deeply memorised by most TypeScript developers, and rejecting it outright creates unnecessary friction during adoption. Accepting aliases with normalisation means you can use either, and `cofferdam fix` gradually moves a codebase toward the canonical form without a flag day.

---

## Reason field

The reason field is the text after the second `:` in a `cofferdam-ignore` directive:

```ts
// cofferdam-ignore: Warning.TripleEquals: legacy API contract, == is intentional
if (req.status == "200") { ... }
```

**When required:** by default the reason field is required for any check in the `Warning` category. Omitting it produces an info-level diagnostic: `"suppression of Warning.TripleEquals at line N: reason required"`. The finding is still suppressed.

**Per-check override:** a check can declare its own reason-field policy via `OptionSpec`. A check might relax the requirement (`reason = "optional"`) or tighten it (`reason = "required"` for non-Warning categories). See `cofferdam explain <CheckId> --robot` for the `reason_required` field when it lands.

**Why encourage reasons even when not required?** ESLint's decade of experience produced a recognisable anti-pattern: `// eslint-disable-next-line complexity` with no explanation, copy-pasted until the codebase is wallpapered with suppressions nobody understands. The reason field is the primary defence against this. Even for non-Warning checks, a one-clause reason — `"generated file"`, `"vendor code, not our concern"`, `"tracked in JIRA-1234"` — makes the suppression reviewable and the intent clear. Code review can then ask "is this reason still true?" rather than "why does this suppression exist?"

---

## ID-less suppression

```ts
// cofferdam-ignore
someCallWeAreChoosingToIgnoreEntirely();
```

ID-less suppression is accepted and does suppress all findings on the next line (or in the range, or in the file). The engine also emits an info-level `"broad suppression"` diagnostic at that location.

The diagnostic exists because broad suppressions hide future findings. If a new check lands after the suppression was written, it will be silently hidden. ID-specific suppressions (`// cofferdam-ignore: Warning.NoEval`) are unaffected by new checks — they suppress only what they name. Broad suppressions grow in scope as the check catalog grows.

For the same reason, `cofferdam explain` will eventually surface "files with broad suppressions" as a metric, and a future "unused + broad suppression" lint check (separate bead) will help you clean them up.

Prefer ID-specific suppression everywhere. Use ID-less only when you are temporarily silencing an entire noisy location during an audit.

---

## Interaction with baselines

Suppressed findings **never enter the baseline**.

The engine processes suppressions before baseline comparison. A finding that is suppressed on line 42 today will not appear in `cofferdam baseline write` output, so it cannot be baselined. This is intentional: a finding that is both baselined and suppressed is an audit trap — you cannot tell whether it was acknowledged (baselined) or silenced (suppressed), and removing the suppression later does not restore the baseline entry. Keeping the two mechanisms disjoint avoids that confusion.

**Practical consequence:** if you plan to suppress a finding long-term, use a directive. If you plan to acknowledge a class of findings as "pre-existing, fix later", use the baseline. Don't try to do both.

---

## Interaction with `--fail-on`

Suppressed findings **do not count toward the `--fail-on` threshold**.

The engine removes suppressed findings from the issue list before the severity gate is evaluated. This means:

```ts
// cofferdam-ignore: Warning.NoEval: codegen bootstrap
eval(generatedCode);
```

…will not cause exit code 1 even when `--fail-on=info` is set. The suppression is honoured regardless of the fail threshold.

This is the obvious behaviour, but worth stating explicitly to prevent the inverse confusion: "I suppressed the finding but CI is still failing." If CI fails after a suppression is added, a different finding is triggering the gate — run `cofferdam check` locally without `--max-issues` to see the full list.

---

## Conventions

Recommendations for teams adopting suppression directives:

**Keep suppressions ID-specific.** Always name the check being suppressed. ID-less suppression is available but discourages scrutiny of what exactly is being waived.

**Include a reason for non-trivial cases.** Even when the reason field is not required (non-Warning categories), a short clause prevents the "why does this suppression exist?" question in code review. `"third-party type, cannot change"` takes three seconds to write and saves minutes of archaeology later.

**Audit suppressions in code review.** Treat a new `// cofferdam-ignore` the same way you treat a new `@ts-ignore`: it's not wrong by default, but it deserves a question. Is the reason still true? Is the scope (next-line vs range vs file) as tight as possible? Is this the right long-term fix, or is it a short-term workaround that should track an issue?

**Prefer next-line over range, and range over file.** Narrower scope means fewer future findings silently hidden. File-wide suppression is appropriate for generated files or vendor code that should not be linted at all; prefer checking the scope is correct before reaching for it.

**Generated and vendor files: exclude them at the config level instead.** If an entire file should never be analysed, list it in `cofferdam.toml`'s `exclude` or in [`.cofferdamignore`](/ignore) rather than reaching for a `cofferdam-ignore-file` directive. Exclusions are visible where someone will look for them; file-wide directives are buried in the file itself.

---

## Comparison with other tools

| Tool | Next-line | Block/range | File-wide | Reason field |
|---|---|---|---|---|
| **cofferdam** (canonical) | `// cofferdam-ignore: <ID>: <reason>` | `// cofferdam-ignore-start: <ID>` … `// cofferdam-ignore-end` | `// cofferdam-ignore-file: <ID>` | Optional (required for Warning) |
| **ESLint** | `// eslint-disable-next-line <rule>` | `/* eslint-disable <rule> */` … `/* eslint-enable <rule> */` | `/* eslint-disable <rule> */` at top | Not supported (plugin can enforce) |
| **Biome** | `// biome-ignore <rule>: <reason>` | Not supported natively | Not supported natively | Required |
| **Ruff** | `# noqa: <code>` | Not supported | `# ruff: noqa: <code>` at top | Not supported |

cofferdam's canonical form is directly inspired by Biome's design — one primary syntax, reason field built in. The ESLint-style aliases exist to ease migration. The range and file-wide forms extend Biome's coverage for the cases Biome delegates to IDE tooling.
