# Contributing to cofferdam

## How this project works

**Pull requests are not accepted.**

Cofferdam is a maintainer-driven project. The maintainer reviews all proposed changes and implements them. Community input through issues is welcomed and shapes the direction of the project — the development itself stays under maintainer control to keep the codebase consistent and reviewable.

The workflow is:

1. **File an issue** describing the problem or improvement you have in mind.
2. **Include a proposed solution** — the more specific, the more useful.
3. The maintainer reviews the issue and proposed solution, asks follow-up questions if needed, and implements accepted proposals.

## Issue templates

The `.github/ISSUE_TEMPLATE/` directory contains structured forms for each issue type. GitHub will present these automatically when you open a new issue.

| Template | Filename | When to use |
|---|---|---|
| Bug report | `bug.yml` | Something is not working correctly — a false positive, a false negative, a crash, or wrong output |
| Feature request | `feature.yml` | An improvement or new capability that is not a new check (e.g. a new output format, a CLI flag, performance) |
| Check request | `check-request.yml` | Proposing a new built-in analysis check |
| Discussion / blank | via `config.yml` | General questions and ideas — redirected to GitHub Discussions |

Blank issues are disabled; all new issues must use one of the three templates above, or open a discussion.

## How to write a good check-request issue

The check-request template (`check-request.yml`) collects the information the maintainer needs to evaluate and implement a proposed check. Here is what each field should contain and why it matters.

### Proposed check ID

Use the `Category.Name` form, for example `Refactor.DeadCode` or `Warning.NoConsoleLog`.

- **Category** must be one of the five: `Consistency`, `Design`, `Readability`, `Refactor`, `Warning`.
- **Name** should be concise and in PascalCase.
- The ID is the stable identifier used in output, configuration, and documentation — once a check ships the ID is not renamed.

### Category guidance

| Category | What belongs here |
|---|---|
| `Consistency` | Patterns that should be uniform across a codebase (e.g. import style, quote style) |
| `Design` | Structural concerns that affect maintainability (e.g. too many parameters, duplicate exports) |
| `Readability` | Issues that make code harder to read without being incorrect (e.g. long lines, long functions) |
| `Refactor` | Code that is functionally correct but has unnecessary complexity or duplication |
| `Warning` | Code that is likely a bug or a footgun (e.g. `==` instead of `===`, unreachable code) |

### What it flags

Describe the code pattern precisely. A good description answers:
- What TypeScript construct triggers the check?
- What property of that construct makes it problematic?
- Are there common legitimate uses that should NOT be flagged?

### Example of bad code (flagged)

Provide a minimal, self-contained TypeScript snippet that should trigger the check. Minimal means no boilerplate beyond what is needed to illustrate the pattern.

### Example of good code (not flagged)

Provide the corrected version of the same snippet, or a pattern that looks similar but should pass cleanly. This clarifies the intended boundary of the check.

### Why this matters (optional)

Real-world impact, relevant style guides (e.g. TypeScript ESLint rule names for prior art), or examples from the TypeScript ecosystem. This is not required but helps prioritise.

## For maintainers and automated agents

The recipe for writing a check — scaffolding, AST visitor patterns, required imports, registration, fixtures, and verification steps — is documented in [`CLAUDE.md`](CLAUDE.md). That file is the authoritative implementation guide.
