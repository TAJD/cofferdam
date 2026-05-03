# Output formats

`cofferdam check` ships three rendering modes. Pick by `--format=<text|json|compact>`. The shorthand `--robot` defaults to `--format=json` when no `--format` is set; otherwise the explicit `--format` wins.

| Format    | Audience                            | Schema                | Byte-economy* |
|-----------|-------------------------------------|-----------------------|---------------|
| `text`    | Humans, terminal output (default)   | Free-form, decorative | 1.00× (baseline) |
| `json`    | CI pipelines, full-fidelity tooling | Stable, documented    | 1.68× of text |
| `compact` | AI agents, prompt context-shovelling | Stable, line-oriented | 0.94× of text, 0.56× of JSON |

\* Measured on a 509-finding run against `bestefforttools`. Numbers will shift as checks land or messages change; the relative ordering is what matters.

## `text` format

Default. Findings grouped by category, priority-sorted within each. Decorated with category headers and a trailing summary line. Use for humans reading reports in a terminal.

```text
── Warning ───────────────
  [ 15] [    high] src/auth.ts:42:7  use `===` instead of `==`  (Warning.TripleEquals)

1 finding(s)
```

Honors `--quiet` (suppresses the summary line and the "no findings" line). Emits no ANSI escape codes today, so `NO_COLOR` is trivially honored.

## `json` format

Stable schema, machine-readable. The full contract is the canonical source for tooling integrations. Pretty-print with `--pretty`.

```jsonc
{
  "summary": {
    "total": 1,
    "by_category": { "warning": 1 }
  },
  "findings": [
    {
      "id": "Warning.TripleEquals",
      "category": "warning",
      "priority": 15,
      "severity": "high",
      "file": "src/auth.ts",
      "line": 42,
      "column": 7,
      "start_byte": 800,
      "end_byte": 806,
      "message": "use `===` instead of `==`"
    }
  ]
}
```

Optional fields appear only when relevant — `summary.new` / `summary.baselined` / per-finding `baselined` show up when a baseline is active; `summary.truncated_from` shows up only under `--max-issues`. Schema additions are non-breaking; field renames or removals are.

## `compact` format

Pipe-delimited line-per-finding format. Designed for the case where an AI agent shovels findings into a prompt and pays per byte. Header line followed by one record per finding.

```text
priority|severity|category|id|file|line|column|message
15|high|warning|Warning.TripleEquals|src/auth.ts|42|7|use `===` instead of `==`
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
- **Related spans.** Cross-file findings (`Design.DuplicateExportName`, `Refactor.DuplicateBlock`) emit only the primary location in compact mode. The "also at" locations are JSON-only.
- **Truncation note.** `--max-issues` truncates the rendered findings; compact mode does not surface the original total. Use `--format=json` (which adds `summary.truncated_from`) when truncation matters.

These are deliberate v1 cuts to keep the schema fixed at eight columns. If a future use case demands them, the path forward is a `--format=compact-v2` opt-in rather than silently widening the existing schema.

### Byte-economy

Measured against the same 509-finding run on `bestefforttools`:

- `text`: 87,548 bytes (baseline)
- `json` (compact, no `--pretty`): 146,882 bytes — **1.68× text**
- `compact`: 81,831 bytes — **0.94× text**, **0.56× JSON**

Compact's win over JSON (≈44% bytes saved) is structural: no field-name repetition, no quoting noise, no empty `summary` object. The win over text is small because cofferdam's text formatter is already information-dense — one line per finding with no decorative padding. **The real value of compact mode is parsing simplicity for agents**, not raw byte savings: `splitn(8, '|')` beats running a JSON parser when you're pushing context into an LLM call.

## Picking a format

- **Local terminal use** → `text` (default). Skip everything else.
- **CI / tooling integrations / programmatic consumers** → `json`. Full schema, baseline-aware, related spans, truncation metadata.
- **AI-agent prompt shovelling** → `compact` when token economy is the priority and you don't need baseline tags or related spans; `json` when you do.

`--robot --format=compact` is the canonical AI-agent invocation.
