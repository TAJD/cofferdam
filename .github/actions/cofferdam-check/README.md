# cofferdam-check

Run [cofferdam](https://github.com/TAJD/cofferdam) against your TypeScript codebase and surface findings as native GitHub workflow annotations.

## Quickstart

```yaml
name: cofferdam

on: [push, pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: ./.github/actions/cofferdam-check
        with:
          paths: src/
          fail-on: high
```

## Inputs

| Name       | Default                       | Description                                                                 |
|------------|-------------------------------|-----------------------------------------------------------------------------|
| `paths`    | `.`                           | Space-separated list of files or directories to analyze.                    |
| `fail-on`  | `high`                        | Severity threshold for exit-1. One of: `low`, `medium`, `high`.            |
| `baseline` | `.cofferdam/baseline.json`    | Path to a baseline file. Pass empty string to disable baseline detection.   |
| `since`    | `` (empty)                    | PR mode — only check files changed since this git ref. Empty = scan all.   |
| `version`  | `latest`                      | Release tag to download, e.g. `v0.2.0`. `latest` resolves automatically.   |
| `config`   | `cofferdam.toml`              | Path to a `cofferdam.toml` config file. Empty = auto-discover.              |

## Outputs

None in v1. This is intentional — findings are surfaced entirely through GitHub workflow annotations (inline PR comments and the job summary). A structured `findings-json` output is planned for a future release.

## Notes

- **Binary source**: the action downloads a pre-built release archive from `https://github.com/TAJD/cofferdam/releases`. The requested `version` must exist as a published release tag (`v0.2.0`, `latest`, etc.).
- **Cache key**: the binary is cached under `cofferdam-${version}-${os}-${arch}` using `actions/cache@v4`. A single download is reused across all workflow runs on the same runner OS and version combination.
- **Annotations**: findings are emitted as native GitHub workflow commands (`::error` / `::warning`). They appear as inline annotations on pull requests and in the job log. No SARIF upload or third-party action is required for v1.
- **Threshold logic**: findings at or above `fail-on` emit `::error` and cause the job to exit 1. Findings below the threshold emit `::warning` and are informational only.
- **Baseline support**: pass `baseline: ""` to disable baseline detection entirely. With a baseline active, only findings absent from the baseline trigger the fail-on gate.
