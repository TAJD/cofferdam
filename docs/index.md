---
layout: home

hero:
  name: cofferdam
  tagline: A watertight compartment for your codebase. Isolate bad code, measure it against rules, ship a priority-sorted verdict.
  actions:
    - theme: brand
      text: Get started
      link: /checks
    - theme: alt
      text: View on GitHub
      link: https://github.com/TAJD/cofferdam

features:
  - title: Inspired by Elixir's Credo
    details: Brings the five-category discipline that Credo made popular — Warning, Refactor, Design, Readability, Consistency — to TypeScript. If you've used Credo, the category names and report shape will feel familiar.
  - title: Priority-sorted output
    details: Priority and severity are separate axes. Priority is computed; severity is configured. The report sorts by what to fix first, and CI gates on what must not regress.
  - title: Baseline workflow
    details: Adopt on a legacy codebase without drowning in noise — default mode shows only new findings. Capture the current state with `cofferdam init --baseline`, then tighten over time.
  - title: CI-friendly by design
    details: SARIF output, `--since main` for PR-only mode, GitHub annotations, and a `--robot` flag for machine-readable compact output that AI agents can consume directly.
  - title: Agent-aware
    details: '`cofferdam advise` emits the rules that apply to a file before any edit, so an LLM agent can plan against your layering and complexity limits instead of reverse-engineering them from violations. <a href="/cofferdam/reference/advise">Agent advisory →</a>'
---
