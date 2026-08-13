# Output formats

`cofferdam check` ships four rendering modes. Pick by `--format=<text|json|compact|sarif>`.

**`--robot` flag:** defaults to `--format=json` when no `--format` is set; otherwise the explicit `--format` wins. `--robot` does nothing else — it does not set `--quiet`, does not suppress ANSI (there is none), and does not change exit-code behaviour. Its only effect is the format default. The idiomatic AI-agent invocation is `--robot --format=compact`, which combines the intent signal with the most token-economical output.

| Format    | Audience                                | Schema                | Byte-economy* |
|-----------|-----------------------------------------|-----------------------|---------------|
| `text`    | Humans, terminal output (default)       | Free-form, decorative | 1.00× (baseline) |
| `json`    | CI pipelines, full-fidelity tooling     | Stable, documented    | 1.68× of text |
| `compact` | AI agents, prompt context-shovelling    | Stable, line-oriented | 0.94× of text, 0.56× of JSON |
| `sarif`   | GitHub Code Scanning, Azure DevOps, GitLab, SonarQube, VS Code Sarif Viewer | OASIS SARIF 2.1.0 (fixed) | larger than `json` (rules table + envelope overhead) |

\* Measured on a 509-finding run against `bestefforttools`. Numbers will shift as checks land or messages change; the relative ordering is what matters.

## Priority sorts, severity gates

Priority and severity are two independent axes — the two-axis model
Credo popularised, and easy to miss from a single sentence:

| | Low severity | High severity |
|---|---|---|
| **High priority** | Read first. Does not gate CI. | Read first. Gates CI. |
| **Low priority** | Read last. Does not gate CI. | Read last. Still gates CI. |

**Priority** is computed per finding (category base + check-specific
adjustments) and decides *what to look at first* in the report.
**Severity** is configured per check in `cofferdam.toml` and decides
*what fails the build* via `--fail-on`. A high-priority, low-severity
finding is worth reading before a low-priority, high-severity one — but
only the severity axis blocks CI.

## `text` format

Default. Findings grouped by category, priority-sorted within each. Decorated with category headers and a trailing summary line. Use for humans reading reports in a terminal.

```text
── Refactor ───────────────
  [ 15] [    high] src/auth.ts:42:1  function `handleRequest` has cyclomatic complexity 17, exceeds limit of 15  (Refactor.LongAndComplex)

1 finding(s)
```

Honors `--quiet` (suppresses the summary line and the "no findings" line). Emits no ANSI escape codes today, so `NO_COLOR` is trivially honored.

## `json` format

Stable schema, machine-readable. The full contract is the canonical source for tooling integrations. Pretty-print with `--pretty`.

```jsonc
{
  "summary": {
    "total": 1,
    "by_category": { "refactor": 1 }
  },
  "findings": [
    {
      "id": "Refactor.LongAndComplex",
      "category": "refactor",
      "priority": 15,
      "severity": "high",
      "file": "src/auth.ts",
      "line": 42,
      "column": 1,
      "start_byte": 800,
      "end_byte": 806,
      "message": "function `handleRequest` has cyclomatic complexity 17, exceeds limit of 15"
    }
  ]
}
```

### Field reference

Every field listed below is **stable** — field names and types are part of the contract. Adding new fields is non-breaking; renaming or removing fields, or changing a field's type, requires a major version bump.

**Top-level object**

| Field | Type | Always present | Description |
|-------|------|----------------|-------------|
| `findings` | `array` | Yes | Ordered array of finding objects, sorted by priority descending then check ID ascending. Empty array when no findings. |
| `summary` | `object` | Yes | Aggregate counts for the run. |

**`summary` object**

| Field | Type | Always present | Description |
|-------|------|----------------|-------------|
| `total` | `integer` | Yes | Number of findings in the `findings` array. When `--max-issues` truncation is active, this equals the truncated count (not the full count — see `truncated_from`). |
| `by_category` | `object` | Yes | Map of lowercase category name → finding count. Only categories with at least one finding appear; zero-count categories are omitted. Keys are a subset of: `consistency`, `design`, `readability`, `refactor`, `warning`. |
| `new` | `integer` | No — baseline only | Count of findings **not** matched by the active baseline (i.e., genuinely new since `cofferdam baseline write`). Omitted when no baseline is active. |
| `baselined` | `integer` | No — baseline only | Count of findings matched by the active baseline (these were present when the baseline was written and are acknowledged). Omitted when no baseline is active. |
| `truncated_from` | `integer` | No — truncation only | Total findings produced before `--max-issues` capping. Always strictly greater than `total` when present. Omitted in the common case (no truncation) so the byte output is identical to pre-truncation cofferdam. |

**Per-finding object (each element of `findings`)**

| Field | Type | Always present | Description |
|-------|------|----------------|-------------|
| `id` | `string` | Yes | Dotted check ID, e.g. `Refactor.LongAndComplex`. Stable — safe to use as a map key or filter. |
| `category` | `string` | Yes | Lowercase category: `consistency` \| `design` \| `readability` \| `refactor` \| `warning`. |
| `docs_url` | `string` | Yes | Canonical docs-catalogue URL for this check, e.g. `https://tajd.github.io/cofferdam/checks/Refactor.LongAndComplex`. Derived from `id` — no per-check configuration needed. |
| `priority` | `integer` | Yes | Computed sort priority in the range `-20..=20`. Higher value = surfaces first. Not configurable; derived by the engine. |
| `severity` | `string` | Yes | Configured severity: `info` \| `low` \| `medium` \| `high` \| `critical`. Matches `--fail-on=<level>` threshold values. |
| `file` | `string` | Yes | Path to the file containing the finding. Forward-slash normalized (even on Windows) so it is safe to use as an editor link or CLI argument on any platform. |
| `line` | `integer` | Yes | 1-based line number of the finding's primary span. |
| `column` | `integer` | Yes | 1-based column number of the finding's primary span. |
| `start_byte` | `integer` | Yes | Byte offset of the span start into the file's UTF-8 text. Useful for span-based autofix or highlighting without re-parsing. |
| `end_byte` | `integer` | Yes | Byte offset of the span end (exclusive). |
| `message` | `string` | Yes | Human-readable finding description. May contain backtick-quoted identifiers. |
| `related` | `array` | No — multi-location only | Additional locations participating in the same finding (e.g. the other files where a duplicate export or duplicate block appears). Omitted entirely (not present, not `[]`) when the finding is single-location. See sub-fields below. |
| `baselined` | `boolean` | No — baseline only | `true` when this finding matched an entry in the active baseline; `false` when it did not. Omitted entirely when no baseline is active, so the JSON schema for a no-baseline run stays byte-identical to pre-baseline cofferdam. |

**`related` array element sub-fields**

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | Path to the related file, forward-slash normalized. |
| `line` | `integer` | 1-based line of the related span. |
| `column` | `integer` | 1-based column of the related span. |
| `start_byte` | `integer` | Byte offset of the related span start. |
| `end_byte` | `integer` | Byte offset of the related span end (exclusive). |

Optional fields appear only when relevant — `summary.new` / `summary.baselined` / per-finding `baselined` show up when a baseline is active; `summary.truncated_from` shows up only under `--max-issues`. Schema additions are non-breaking; field renames or removals are.

## `compact` format

Pipe-delimited line-per-finding format. Designed for the case where an AI agent shovels findings into a prompt and pays per byte. Header line followed by one record per finding.

```text
priority|severity|category|id|file|line|column|message
15|high|refactor|Refactor.LongAndComplex|src/auth.ts|42|1|function `handleRequest` has cyclomatic complexity 17, exceeds limit of 15
```

### Schema (stable contract)

Header is fixed at exactly eight `|`-separated columns:

```
priority|severity|category|id|file|line|column|message
```

The columns are stable. Adding a column would break parsers that hardcode the count, so the schema does not grow inline — see "Limitations" below for the trade-offs and what to do when you need extra fields.

### Parsing rules

- **Message is the last column.** Any embedded `|` characters in a message render naked. Use `splitn(8, '|')` (Rust), `split('|', 8)` (Python `str.split`), or your language's "split on first N delimiters" primitive — take the rest of the line as the message.
- **Lines are LF-delimited.** Newlines inside a message are collapsed to spaces so the line-per-finding contract is unconditional.
- **Empty result is well-formed.** When zero findings are emitted, the output is just the header line followed by a newline. Parsers that always read the first line for column names won't crash on a clean run.
- **Paths are forward-slashed.** Same convention as the other formats — agent-friendly, copy-paste-able as an editor link.

### Severity column values

`info` | `low` | `medium` | `high` | `critical`. Same set as `--fail-on=<level>`.

### Category column values

`consistency` | `design` | `readability` | `refactor` | `warning`. Lowercased. (Or `unknown` for malformed check IDs — practically never happens for built-in checks.)

### Limitations

Compact mode v1 does not carry:

- **Baseline tags.** When a baseline is active, the per-finding `baselined` flag is dropped from compact output. Use `--format=json` if you need to know which findings are baselined vs new. The CI gate (`--fail-on`) still respects baselines correctly — only the *display* drops the tag.
- **Related spans.** Cross-file findings (`Design.DuplicateExportName`, `Refactor.NearDuplicateBlock`) emit only the primary location in compact mode. The "also at" locations are JSON-only.
- **Truncation note.** `--max-issues` truncates the rendered findings; compact mode does not surface the original total. Use `--format=json` (which adds `summary.truncated_from`) when truncation matters.

These are deliberate v1 cuts to keep the schema fixed at eight columns. If a future use case demands them, the path forward is a `--format=compact-v2` opt-in rather than silently widening the existing schema.

### Byte-economy

Measured against the same 509-finding run on `bestefforttools`:

- `text`: 87,548 bytes (baseline)
- `json` (compact, no `--pretty`): 146,882 bytes — **1.68× text**
- `compact`: 81,831 bytes — **0.94× text**, **0.56× JSON**

Compact's win over JSON (≈44% bytes saved) is structural: no field-name repetition, no quoting noise, no empty `summary` object. The win over text is small because cofferdam's text formatter is already information-dense — one line per finding with no decorative padding. **The real value of compact mode is parsing simplicity for agents**, not raw byte savings: `splitn(8, '|')` beats running a JSON parser when you're pushing context into an LLM call.

## `sarif` format

[SARIF (Static Analysis Results Interchange Format)](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html) is the OASIS-standard JSON schema for static-analysis tool output and the format GitHub Code Scanning ingests natively. `--format=sarif` emits a SARIF 2.1.0 document suitable for direct upload via [`github/codeql-action/upload-sarif`](https://github.com/github/codeql-action) — see the [CI recipe](ci-recipes.md#6-sarif-upload-to-github-code-scanning). The same document is consumed by Azure DevOps, GitLab Vulnerability Reports, SonarQube, and the VS Code Sarif Viewer.

Pretty-print with `--pretty` (defaults to compact, single-line JSON).

```bash
cofferdam check --format sarif --pretty src/
```

Mapping from cofferdam concepts to SARIF:

| cofferdam               | SARIF                                                              |
|-------------------------|--------------------------------------------------------------------|
| `Issue.check_id`        | `runs[].results[].ruleId`                                          |
| `Issue.message`         | `runs[].results[].message.text`                                    |
| `Issue.file`            | `runs[].results[].locations[].physicalLocation.artifactLocation.uri` (forward-slashed) |
| `Issue.span.line`       | `runs[].results[].locations[].physicalLocation.region.startLine`   |
| `Issue.span.column`     | `runs[].results[].locations[].physicalLocation.region.startColumn` |
| `Issue.span.start_byte` | `runs[].results[].locations[].physicalLocation.region.byteOffset`  |
| `Issue.span.end_byte` − `start_byte` | `runs[].results[].locations[].physicalLocation.region.byteLength` |
| `Issue.related[]`       | `runs[].results[].relatedLocations[]`                              |
| `CheckMeta.id`          | `runs[].tool.driver.rules[].id` (one entry per unique check id seen) |
| `CheckMeta.explanation` | `runs[].tool.driver.rules[].shortDescription.text` and `.fullDescription.text` |
| `CheckMeta.category`    | `runs[].tool.driver.rules[].properties.category` (also surfaced as a tag) |
| `CheckMeta.base_priority` | `runs[].tool.driver.rules[].properties.priority`                 |

### Severity → SARIF level

SARIF has a coarser scale than cofferdam (`note | warning | error | none`). The fixed mapping is:

| cofferdam `severity` | SARIF `level` |
|----------------------|---------------|
| `info`               | `note`        |
| `low`                | `note`        |
| `medium`             | `warning`     |
| `high`               | `error`       |
| `critical`           | `error`       |

The `level` is emitted both per-rule (`defaultConfiguration.level`) and per-result (`level`) so consumers that respect either field render correctly.

### Fingerprints (cross-run finding identity)

Every result carries a `partialFingerprints["cofferdam/v1"]` entry — a stable hex digest of `(check_id, file, line, message)`. GitHub Code Scanning (and other consumers) use this to keep the same finding pinned to the same alert across runs even when surrounding line numbers shift. The `cofferdam/v1` namespace versions the algorithm; if the inputs ever change, the version bumps so existing alerts don't all re-trigger.

### Limitations

SARIF mode v1 does not carry:

- **Baseline tags.** SARIF has no native field for "this finding was already present at baseline write time." GitHub Code Scanning and most other UIs track new vs. acknowledged via their own dismissal model on top of `partialFingerprints` — that's the SARIF-native answer. Use `--format=json` when you need the per-finding `baselined` flag in the output.
- **Truncation note.** `--max-issues` truncates the rendered results without a separate `truncated_from` field. Use `--format=json` when truncation accounting matters.

The CI gate (`--fail-on`) is unaffected — it always considers the full pre-truncation set.

## Picking a format

- **Local terminal use** → `text` (default). Skip everything else.
- **CI / tooling integrations / programmatic consumers** → `json`. Full schema, baseline-aware, related spans, truncation metadata.
- **AI-agent prompt shovelling** → `compact` when token economy is the priority and you don't need baseline tags or related spans; `json` when you do.
- **GitHub Code Scanning, GitLab Vulnerability Reports, Azure DevOps, SonarQube, VS Code Sarif Viewer** → `sarif`. Standard schema, no per-platform glue code.

`--robot --format=compact` is the canonical AI-agent invocation.

## Future formats

These formats are planned but unimplemented. Passing `--format=github-annotations` today will produce an error.

### `github-annotations` (planned)

GitHub Actions supports [workflow commands](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/workflow-commands-for-github-actions#setting-a-warning-message) that annotate pull-request diffs inline. The `github-annotations` format will emit one line per finding in the format:

```
::warning file=src/auth.ts,line=42,col=1,endLine=42,endColumn=1::Refactor.LongAndComplex: function `handleRequest` has cyclomatic complexity 17, exceeds limit of 15
```

This is line-oriented and intended to be piped through `echo` inside a GitHub Actions step, producing annotations that appear directly in the PR file view. Like compact, it will not carry related spans or baseline tags (JSON is the right choice when you need those). Severity-to-level mapping: `info` → `::notice`, `low`/`medium` → `::warning`, `high`/`critical` → `::error`.
