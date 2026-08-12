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
| Findings on the PR + Security tab via SARIF | [§6 SARIF upload](#6-sarif-upload-to-github-code-scanning) |
| Catching regressions in built HTML output | [§7 verify --dist](#_7-verifying-build-output-verify-dist) |
| Validating `.cofferdam/knowledge/*.md` notes | [§8 context --lint-knowledge](#_8-validating-knowledge-notes-context---lint-knowledge) |
| Gating on one check only (e.g. a plugin convention) | [§9 `--only`](#_9-gating-on-a-single-check-with-only) |

Each recipe is shown for GitHub Actions first because it's the most common; GitLab / CircleCI / Drone equivalents are at the bottom.

## Universal flags

Every `cofferdam check` flag, with its default and its exact effect on the gate, is in [the CLI reference](/reference/cli#cofferdam-check) — generated from the binary, so it cannot drift. The ones that come up in nearly every CI job are `--fail-on`, `--baseline`, `--since`, `--format` and `--only`; each has a recipe below.

One caveat the reference cannot state, because it is about how you use the tool rather than what it does: the findings cache can mask the change you are testing. See [below](#the-cache-can-mask-the-change-you-are-testing).

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
      - run: npx --yes @cofferdam/cofferdam check
```

`npx --yes @cofferdam/cofferdam check` walks the current directory and exits 1 on any finding at `medium` severity or higher. No config required. Baselined findings (if `.cofferdam/baseline.json` is committed) never trigger the gate.

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
      - run: npx --yes @cofferdam/cofferdam check --since=origin/${{ github.base_ref }}
        if: github.event_name == 'pull_request'
      - run: npx --yes @cofferdam/cofferdam check
        if: github.event_name == 'push'
```

The `if:` guards split the job into two paths: PRs check only the diff against the base branch; pushes to `main` check the full repo.

### 3. Baseline pattern: only fail on new findings

For repos adopting cofferdam without a clean rewrite. Capture once, commit, then CI fails only on findings absent from the baseline.

**One-time setup** (locally):

```bash
npx @cofferdam/cofferdam init --baseline   # writes cofferdam.toml + .cofferdam/baseline.json
git add cofferdam.toml .cofferdam/baseline.json
git commit -m "chore: adopt cofferdam with baseline"
```

**CI workflow** is unchanged from §1 — `cofferdam check` auto-detects `.cofferdam/baseline.json` and treats baselined findings as non-gating. Pass `--fail-on-new` if you want to make the intent explicit in CI logs:

```yaml
      - run: npx --yes @cofferdam/cofferdam check --fail-on-new
```

When you fix a baselined finding, regenerate:

```bash
npx @cofferdam/cofferdam baseline write
git commit -am "chore(cofferdam): refresh baseline after fixing X"
```

### 4. Robot mode for AI review bots

Pipe `--robot --format=compact` output into an AI review step. Compact format is line-delimited (`splitn(8, '|')`-friendly) and ~44% smaller than JSON.

```yaml
      - name: Run cofferdam (compact)
        id: cofferdam
        run: |
          npx --yes @cofferdam/cofferdam check --robot --format=compact > findings.txt
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
      - run: npx --yes @cofferdam/cofferdam check --robot --format=json > findings.json
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

#### The cache can mask the change you are testing

Separately from the npm cache above, cofferdam keeps a **findings cache** in
`.cofferdam/cache/`, keyed on `(content, config, engine_version)`. In CI this is
almost always what you want.

It bites in one situation: evaluating a change to cofferdam itself, or to a
plugin, against a repo you have already scanned with the *same* version. The key
does not change, so the cached findings replay and the run appears to prove that
your change did nothing.

Pass `--no-cache` whenever you are validating a rule change:

```bash
cofferdam check apps/web/src --no-cache
```

Releases self-heal — the version bump invalidates the cache directory — so this
only affects developers re-running one version against an already-scanned
checkout.

### 6. SARIF upload to GitHub Code Scanning

Emit SARIF 2.1.0 and let GitHub render findings on the PR diff and in the **Security → Code scanning** tab. No bespoke parsing — `github/codeql-action/upload-sarif` consumes the format directly.

```yaml
# .github/workflows/cofferdam-sarif.yml
name: cofferdam (SARIF)
on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read
  security-events: write   # required for upload-sarif

jobs:
  scan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
      - name: Run cofferdam (SARIF)
        run: npx --yes @cofferdam/cofferdam check --format=sarif > cofferdam.sarif
        continue-on-error: true        # don't block the upload on a non-zero exit
      - uses: github/codeql-action/upload-sarif@v3
        with:
          sarif_file: cofferdam.sarif
          category: cofferdam          # distinguishes from CodeQL or other SARIF uploads
```

After the workflow runs once on `main`, findings show up:

- **Pull-request files view** — annotated inline next to the offending lines.
- **Security tab → Code scanning alerts** — full list with state (open / fixed / dismissed) tracked across runs via cofferdam's `partialFingerprints`.

Notes:

- `permissions.security-events: write` is mandatory; without it the upload step 403s.
- `continue-on-error: true` on the cofferdam step lets the SARIF upload happen even when `--fail-on=<level>` would otherwise exit 1. The findings still drive whatever PR-required-status checks you wire into the Security policies.
- `category: cofferdam` lets you run cofferdam alongside CodeQL or other SARIF emitters without findings overwriting each other.
- For private repos, GitHub Code Scanning requires GitHub Advanced Security; public repos get it for free.

If you want the gate exit code to also fail the workflow (in addition to driving alerts), drop `continue-on-error: true` and accept that the SARIF upload step won't run on red builds — or add a separate `cofferdam check` step (no `--format=sarif`) before the SARIF emitter and let the upload step run with `if: always()`.

### 7. Verifying build output (`verify --dist`)

`cofferdam check` never looks at build output — [`cofferdam verify
--dist`](/verify-dist) is a separate, opt-in subcommand for catching
regressions that only exist in *rendered* HTML (a templating bug that
duplicates a `<title>` across routes, a build step that drops `lang` from
`<html>`). Run it as its own step, after the build, pointed at the build
output directory:

```yaml
      - run: npm run build
      - run: npx --yes @cofferdam/cofferdam verify --dist dist/
```

It only runs checks explicitly tagged output-mode-eligible (see
[Verifying built output](/verify-dist)), so it's safe to add alongside a
normal `cofferdam check` step without double-reporting the same finding
twice.

### 8. Validating knowledge notes (`context --lint-knowledge`)

[`cofferdam context`](/reference/context) is advisory and always exits 0 —
with one deliberate exception. `--lint-knowledge` validates every
`.cofferdam/knowledge/*.md` note instead of producing a digest: selectors
must parse, and every selector must match at least one file in the repo.
It exits nonzero when they don't, which catches a broken glob or an orphan
selector left behind by a refactor — a note that silently stops being
surfaced to agents is worse than no note at all.

```yaml
      - name: Validate knowledge notes
        run: npx --yes @cofferdam/cofferdam context --lint-knowledge
```

It ignores `paths` / `--staged` / `--base` / `--budget` / `--format`, needs
no git history, and is fast enough to run as its own step next to
`cofferdam check`. See [knowledge notes](/knowledge-notes) for the note
format.

### 9. Gating on a single check with `--only`

A repo often wants one project-specific rule to fail the build without turning on the
full suite — a plugin check enforcing a local convention, say, in a directory that
already carries unrelated findings. `--only` restricts the run to one check:

```yaml
      - name: Design system convention
        run: |
          pnpm --filter @acme/cofferdam-design-system build
          npx --yes @cofferdam/cofferdam check apps/web/src --only Warning.DesignSystemConvention
```

The flag applies after plugin merge, so it covers plugin-emitted checks as well as
built-ins. Budgets, the baseline and the exit-code gate all see only the named check's
findings.

The exit codes are the point:

| Exit | Meaning |
|---|---|
| 0 | The named check found nothing |
| 1 | The named check found something at or above `--fail-on` |
| 2 | No check by that name exists |

Exit 2 is what makes this worth using over a hand-rolled filter. The obvious
alternative is to run the whole suite and grep the JSON:

```sh
# don't do this
cofferdam check apps/web/src --format json \
  | jq -e '[.findings[] | select(.check == "DesignSystemConvention")] | length == 0'
```

A filter over findings cannot tell "no violations" from "the check never ran". If the
plugin fails to load — an unbuilt `dist/`, a bad path in `cofferdam.toml`, a typo in
the check id — the findings array contains nothing matching, and the gate reports
success against a check that never executed. This is not hypothetical: the projektor
repo ran such a wrapper against a plugin whose build step CI never invoked. It printed
`OK` and exited 0 on every pull request for as long as it was wired that way.

`--only` fails loudly instead. A gate that cannot find its check exits 2.

Build the plugin in the same step, as above. `dist/` directories are usually
gitignored, so a fresh CI checkout has no plugin until something builds it.

## GitLab CI

```yaml
# .gitlab-ci.yml
cofferdam:
  image: node:20
  script:
    - npx --yes @cofferdam/cofferdam check
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
    - npx --yes @cofferdam/cofferdam check --since=$CI_MERGE_REQUEST_DIFF_BASE_SHA
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
      - run: npx @cofferdam/cofferdam check
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

PR-only: replace `npx @cofferdam/cofferdam check` with `npx @cofferdam/cofferdam check --since=origin/main` (or `origin/$CIRCLE_BRANCH` when you have it).

## Drone / Woodpecker / generic

Any CI that runs a Linux container with `node:20` works. Minimal:

```yaml
# .drone.yml or .woodpecker.yml
steps:
  cofferdam:
    image: node:20
    commands:
      - npx --yes @cofferdam/cofferdam check
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
echo "$files" | xargs npx @cofferdam/cofferdam check --no-baseline --quiet --max-issues=20
```

`--max-issues=20` caps the displayed list so a noisy commit doesn't flood the terminal. The gate still considers the full set.

## Tuning checklist

Before shipping a CI integration, walk through:

1. **Severity threshold.** Default `--fail-on=medium`. Demote to `low` when you want CI to gate on the Refactor / Readability rules; promote to `high` if you only want the security-tier (`Warning.NoEval`, `Warning.TripleEquals`) to break the build.
2. **Baseline.** Capture once on a legacy adoption (`cofferdam init --baseline`), then refresh after each cleanup run (`cofferdam baseline write`).
3. **PR-only vs. full-repo.** PR-only (`--since`) for fast feedback on contributors; full-repo on the default branch to catch regressions on shared code paths.
4. **Output format.** `text` for human terminal logs, `json` for tooling integrations (PR-comment bots, dashboards), `compact` when the consumer is an LLM that pays per token, `sarif` for GitHub Code Scanning + other static-analysis consumers.
5. **Caching.** `actions/setup-node@v4 with: cache: npm` (or equivalent) handles the postinstall binary download for free.

## Pre-flight checks

Before wiring up a new CI pipeline, run [`cofferdam doctor --robot`](doctor.md) as a fast pre-flight step to verify the binary, config, baseline, and git integration are all in good shape. A non-zero exit from `doctor` means something is misconfigured that will cause `cofferdam check` to behave unexpectedly.

```yaml
      - name: Pre-flight
        run: npx --yes @cofferdam/cofferdam doctor --robot
```

`doctor` exits 0 on warns (so a missing baseline or unknown config key doesn't break CI) and exits 1 only when a hard failure is detected (missing git, corrupt baseline, binary version mismatch).

## What's not yet here

- **Self-hosted runner notes** — cofferdam's npm postinstall downloads a Rust binary, which an air-gapped runner cannot do. The mechanism is documented under [air-gapped installs](/install#air-gapped-installs) — `COFFERDAM_SKIP_DOWNLOAD=1` plus a pre-staged binary at `COFFERDAM_BINARY_PATH`, or `cargo install` from source. What is missing here is a worked CI recipe wrapping that, not the capability.
