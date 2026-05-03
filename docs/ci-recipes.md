# CI integration recipes

Drop-in workflows for the major CI systems. Each recipe runs `cofferdam check`, gates the build on `--fail-on=<level>`, and uses the project's baseline (when present) so adopting cofferdam on legacy code doesn't immediately turn CI red.

## Recipe selector

| You want… | Use |
|---|---|
| Plain "fail the build on new findings" | [GitHub Actions §1](#1-minimal-fail-the-build-on-medium-or-higher) — applies to every system |
| PR-only review (only check changed files) | [§2 PR-only mode](#2-pr-only-mode-with---since) |
| Baseline-tracked rollout on a legacy repo | [§3 Baseline pattern](#3-baseline-pattern-only-fail-on-new-findings) |
| AI-agent-friendly output for review bots | [§4 Robot mode](#4-robot-mode-for-ai-review-bots) |
| Faster builds via caching | [§5 Caching](#5-caching) |

Each recipe is shown for GitHub Actions first because it's the most common; GitLab / CircleCI / Drone equivalents are at the bottom.

## Universal flags

A short cheat sheet for the flags that matter in CI. Full reference: [`docs/output-formats.md`](output-formats.md), [`docs/checks/`](checks/), or `cofferdam check --help`.

| Flag | Purpose |
|---|---|
| `--fail-on=<level>` | Severity threshold for exit-1 gate. `info` / `low` / `medium` / `high` / `critical`. Default `medium`. |
| `--baseline=<path>` | Active baseline file. Auto-detected at `.cofferdam/baseline.json` when present. |
| `--no-baseline` | Disable baseline detection entirely for this run. |
| `--since=<git-ref>` | PR-only mode — only check files changed in `<git-ref>...HEAD`. |
| `--robot` | Default to a machine-readable format. Pairs with `--format=compact` for AI shovelling. |
| `--format=<text\|json\|compact>` | Output format. `text` for humans, `json` for tools, `compact` for AI agents. |
| `--max-issues=<N>` | Cap rendered findings (gate still uses the full set). |
| `--quiet` | Suppress info lines (decorative output, not findings). |

## GitHub Actions

### 1. Minimal: fail the build on medium-or-higher

```yaml
# .github/workflows/cofferdam.yml
name: cofferdam
on:
  push:
    branches: [main]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npx --yes cofferdam check
```

`npx --yes cofferdam check` walks the current directory and exits 1 on any finding at `medium` severity or higher. No config required. Baselined findings (if `.cofferdam/baseline.json` is committed) never trigger the gate.

### 2. PR-only mode with `--since`

Only check files changed in the PR. Safe to drop into a noisy legacy repo: pre-existing findings on untouched files are ignored entirely.

```yaml
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0   # cofferdam needs full history to diff against the base
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - run: npx --yes cofferdam check --since=origin/${{ github.base_ref }}
        if: github.event_name == 'pull_request'
      - run: npx --yes cofferdam check
        if: github.event_name == 'push'
```

The `if:` guards split the job into two paths: PRs check only the diff against the base branch; pushes to `main` check the full repo.

### 3. Baseline pattern: only fail on new findings

For repos adopting cofferdam without a clean rewrite. Capture once, commit, then CI fails only on findings absent from the baseline.

**One-time setup** (locally):

```bash
npx cofferdam init --baseline   # writes cofferdam.toml + .cofferdam/baseline.json
git add cofferdam.toml .cofferdam/baseline.json
git commit -m "chore: adopt cofferdam with baseline"
```

**CI workflow** is unchanged from §1 — `cofferdam check` auto-detects `.cofferdam/baseline.json` and treats baselined findings as non-gating. Pass `--fail-on-new` if you want to make the intent explicit in CI logs:

```yaml
      - run: npx --yes cofferdam check --fail-on-new
```

When you fix a baselined finding, regenerate:

```bash
npx cofferdam baseline write
git commit -am "chore(cofferdam): refresh baseline after fixing X"
```

### 4. Robot mode for AI review bots

Pipe `--robot --format=compact` output into an AI review step. Compact format is line-delimited (`splitn(8, '|')`-friendly) and ~44% smaller than JSON.

```yaml
      - name: Run cofferdam (compact)
        id: cofferdam
        run: |
          npx --yes cofferdam check --robot --format=compact > findings.txt
          echo "count=$(tail -n +2 findings.txt | wc -l)" >> "$GITHUB_OUTPUT"
        continue-on-error: true
      - name: Render to step summary
        if: always()
        run: |
          {
            echo "## Cofferdam — ${{ steps.cofferdam.outputs.count }} finding(s)"
            echo
            echo '```'
            cat findings.txt
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Fail if findings
        if: steps.cofferdam.outputs.count != '0'
        run: exit 1
```

`continue-on-error` lets the step summary render even when the gate would normally fail the run; the explicit `exit 1` step at the end re-applies the gate. The header line counts as line 1, so `tail -n +2` skips it.

For full-fidelity JSON (with baseline tags, related spans, truncation metadata):

```yaml
      - run: npx --yes cofferdam check --robot --format=json > findings.json
```

### 5. Caching

`cofferdam` ships as an npm package that downloads a Rust binary on `postinstall`. Cache `~/.npm` and the global cofferdam install to skip the download on subsequent runs.

```yaml
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: npm                # caches ~/.npm by lockfile hash
```

If your project already has a `package.json` and `package-lock.json` with cofferdam as a `devDependency`, the standard `npm ci` cache key handles cofferdam's binary too — no extra config needed.

## GitLab CI

```yaml
# .gitlab-ci.yml
cofferdam:
  image: node:20
  script:
    - npx --yes cofferdam check
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
      variables:
        BASE: $CI_MERGE_REQUEST_DIFF_BASE_SHA
      changes:
        - "**/*.{ts,tsx,mts,cts}"
  cache:
    key:
      files:
        - package-lock.json
    paths:
      - node_modules/
      - .npm/
```

PR-only mode:

```yaml
  script:
    - npx --yes cofferdam check --since=$CI_MERGE_REQUEST_DIFF_BASE_SHA
```

GitLab gives you the merge-request base SHA in `$CI_MERGE_REQUEST_DIFF_BASE_SHA`. Outside MR pipelines (push to default branch), drop the flag to scan the full repo.

Robot mode → MR comment via the GitLab API: capture compact output to a file, then post via `curl` from the same job. Pattern is the same as the GitHub step-summary recipe.

## CircleCI

```yaml
# .circleci/config.yml
version: 2.1
jobs:
  cofferdam:
    docker:
      - image: cimg/node:20.0
    steps:
      - checkout
      - restore_cache:
          keys:
            - cofferdam-v1-{{ checksum "package-lock.json" }}
      - run: npm ci
      - run: npx cofferdam check
      - save_cache:
          key: cofferdam-v1-{{ checksum "package-lock.json" }}
          paths:
            - ~/.npm
            - node_modules
workflows:
  check:
    jobs:
      - cofferdam
```

PR-only: replace `npx cofferdam check` with `npx cofferdam check --since=origin/main` (or `origin/$CIRCLE_BRANCH` when you have it).

## Drone / Woodpecker / generic

Any CI that runs a Linux container with `node:20` works. Minimal:

```yaml
# .drone.yml or .woodpecker.yml
steps:
  cofferdam:
    image: node:20
    commands:
      - npx --yes cofferdam check
```

## Pre-commit hook

Run cofferdam locally before each commit. Two options:

### Husky + lint-staged

```jsonc
// package.json
{
  "lint-staged": {
    "*.{ts,tsx,mts,cts}": "cofferdam check --no-baseline --quiet"
  }
}
```

`lint-staged` appends the staged file paths to the command, so cofferdam runs against exactly the files about to be committed — no need for `--since`. `--no-baseline` is recommended at the pre-commit stage so legacy baselined findings don't quietly slip through. `--quiet` keeps the output terse.

### Plain pre-commit hook

```bash
#!/usr/bin/env bash
# .git/hooks/pre-commit
set -e
files=$(git diff --cached --name-only --diff-filter=AMR -- '*.ts' '*.tsx' '*.mts' '*.cts')
[ -z "$files" ] && exit 0
echo "$files" | xargs npx cofferdam check --no-baseline --quiet --max-issues=20
```

`--max-issues=20` caps the displayed list so a noisy commit doesn't flood the terminal. The gate still considers the full set.

## Tuning checklist

Before shipping a CI integration, walk through:

1. **Severity threshold.** Default `--fail-on=medium`. Demote to `low` when you want CI to gate on the Refactor / Readability rules; promote to `high` if you only want the security-tier (`Warning.NoEval`, `Warning.TripleEquals`) to break the build.
2. **Baseline.** Capture once on a legacy adoption (`cofferdam init --baseline`), then refresh after each cleanup run (`cofferdam baseline write`).
3. **PR-only vs. full-repo.** PR-only (`--since`) for fast feedback on contributors; full-repo on the default branch to catch regressions on shared code paths.
4. **Output format.** `text` for human terminal logs, `json` for tooling integrations (PR-comment bots, dashboards), `compact` when the consumer is an LLM that pays per token.
5. **Caching.** `actions/setup-node@v4 with: cache: npm` (or equivalent) handles the postinstall binary download for free.

## Pre-flight checks

Before wiring up a new CI pipeline, run [`cofferdam doctor --robot`](doctor.md) as a fast pre-flight step to verify the binary, config, baseline, and git integration are all in good shape. A non-zero exit from `doctor` means something is misconfigured that will cause `cofferdam check` to behave unexpectedly.

```yaml
      - name: Pre-flight
        run: npx --yes cofferdam doctor --robot
```

`doctor` exits 0 on warns (so a missing baseline or unknown config key doesn't break CI) and exits 1 only when a hard failure is detected (missing git, corrupt baseline, binary version mismatch).

## What's not yet here

- **SARIF output** for GitHub Code Scanning / GitLab Vulnerability Reports — tracked under cd-snv. Recipe will land in this file once SARIF support ships.
- **Self-hosted runner notes** — cofferdam's npm postinstall downloads a Rust binary; air-gapped runners need either a local mirror or a `cargo install` from source. Detailed recipe is a follow-up bead.
