//! `cofferdam agents` — print the agent-onboarding prompt.

use std::process::ExitCode;

/// The version-pinned agent onboarding prompt.
///
/// Deterministic: identical output for the same binary version.
pub fn prompt() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"# cofferdam agents — v{version}

Use cofferdam **before** editing to understand constraints, and **after** to
verify your change is clean. This is the canonical workflow for AI coding agents.

## Before editing a file

```sh
cofferdam advise <file>
```

Prints the architectural layer, invariants, and per-file restrictions from
`cofferdam.invariants.toml` that apply to the target file. Read this before
writing code so you know the constraints upfront.

## Pre-flight a proposed change

```sh
cofferdam advise --diff <git-ref>
```

Reports rules that **would fire** if the change were committed (`would_fire`)
and rules currently firing that the change **clears** (`would_clear`). Run this
before `git commit` to catch violations early.

## Machine-readable findings

```sh
cofferdam check --robot                      # JSON — stable schema
cofferdam check --robot --format=compact     # pipe-delimited, smallest footprint
```

Exit 0: no findings at or above the severity threshold (default: medium).
Exit 1: at least one finding that should be addressed.

## Architectural source of truth

`cofferdam.invariants.toml` at the project root declares layers, boundaries,
and invariants. When a rule fires unexpectedly, read that file first.
Run `cofferdam explain <Check.Id>` for the rationale behind any single check.

## Reporting misbehaviour

False positive, crash, or confusing output? Open an issue at:

  https://github.com/TAJD/cofferdam/issues/new

Include: the command run, `cofferdam --version` output, and a minimal repro.

## MCP

If `cofferdam-mcp` is configured, prefer its tools over CLI shell-out —
structured tool calls without a subprocess, findings stay in context.
It exposes `cofferdam.check`, `cofferdam.advise`, `cofferdam.advise_diff`,
`cofferdam.explain`, and `cofferdam.invariants`. See docs/mcp.md.
"#
    )
}

pub fn run() -> ExitCode {
    print!("{}", prompt());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_version() {
        let p = prompt();
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            p.contains(version),
            "prompt must contain the binary version; got:\n{p}"
        );
    }

    #[test]
    fn prompt_contains_key_commands() {
        let p = prompt();
        assert!(p.contains("cofferdam advise"), "must mention advise");
        assert!(p.contains("--diff"), "must mention advise --diff");
        assert!(p.contains("check --robot"), "must mention check --robot");
        assert!(
            p.contains("cofferdam.invariants.toml"),
            "must mention invariants.toml"
        );
        assert!(
            p.contains("cofferdam explain"),
            "must mention explain command"
        );
    }

    #[test]
    fn prompt_contains_feedback_url() {
        let p = prompt();
        assert!(
            p.contains("https://github.com/TAJD/cofferdam/issues"),
            "must contain the GitHub issues URL"
        );
    }

    #[test]
    fn prompt_mentions_mcp() {
        let p = prompt();
        assert!(p.contains("MCP") || p.contains("mcp"), "must mention MCP");
    }

    #[test]
    fn prompt_is_deterministic() {
        assert_eq!(prompt(), prompt(), "prompt() must be idempotent");
    }
}
