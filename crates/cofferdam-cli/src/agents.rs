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

## Hooks — automate the loop

No MCP server, or want `advise` to fire automatically instead of relying on
the agent to remember? Run:

```sh
cofferdam agents --hooks
```

for a paste-ready Claude Code `settings.json` fragment (plus Cursor and
generic pre-commit equivalents). With hooks wired up, `advise` fires on
every edit and every turn end with no further setup — see docs/hooks.md.
"#
    )
}

/// Paste-ready hook recipes wiring `cofferdam advise` into an agent's edit
/// loop (CD-61). The primary payload is a Claude Code `settings.json`
/// fragment; Cursor rules and a generic pre-commit hook ship as commented
/// equivalents since they don't share Claude Code's JSON hook schema.
pub fn hooks_fragment() -> String {
    r#"# cofferdam hook recipes — paste the relevant block into your tool's config.
#
# ---- Claude Code: .claude/settings.json ----
# PreToolUse fires before Edit/Write/MultiEdit; the hook reads the target
# file path from the event JSON on stdin (`tool_input.file_path`), runs
# `advise` on it, and wraps the output in `hookSpecificOutput.additionalContext`
# so Claude Code injects the advisory into the agent's context before it
# writes. NOTE: a hook's plain stdout is NOT seen by the agent — only the
# `additionalContext` field is; that's why the recipe pipes through jq
# rather than just printing `advise`. (Requires `jq`; the command is `sh`.
# To hard-block an edit instead of advising, exit 2 with the reason on stderr.)
# Stop fires when Claude finishes responding; the second hook here runs
# `advise --diff HEAD` as a pre-commit-style pre-flight check.
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          {
            "type": "command",
            "command": "FILE=$(jq -r '.tool_input.file_path'); cofferdam advise \"$FILE\" | jq -Rs '{hookSpecificOutput:{hookEventName:\"PreToolUse\",permissionDecision:\"allow\",additionalContext:.}}'"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "cofferdam advise --diff HEAD --pretty"
          }
        ]
      }
    ]
  }
}

# ---- Cursor: .cursor/rules (or cursorrules) ----
# Cursor rules are prose, not JSON hooks — paste this as guidance:
#
#   Before editing a file, run `cofferdam advise <file> --format=json` and
#   respect the returned layer/invariant constraints. Before finishing a
#   task, run `cofferdam advise --diff HEAD --pretty` and resolve any
#   would_fire entries.

# ---- Generic pre-commit hook: .git/hooks/pre-commit ----
#
#   #!/bin/sh
#   cofferdam advise --diff HEAD --fail-on=high
"#
    .to_string()
}

pub fn run() -> ExitCode {
    print!("{}", prompt());
    ExitCode::SUCCESS
}

pub fn run_hooks() -> ExitCode {
    print!("{}", hooks_fragment());
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

    #[test]
    fn hooks_fragment_is_valid_json_after_stripping_comments() {
        let frag = hooks_fragment();
        let json_start = frag.find('{').expect("fragment must contain JSON");
        let json_end = frag.rfind('}').expect("fragment must contain JSON") + 1;
        let json_text = &frag[json_start..json_end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_text).expect("Claude Code hooks block must be valid JSON");
        assert!(parsed["hooks"]["PreToolUse"].is_array());
        assert!(parsed["hooks"]["Stop"].is_array());
    }

    #[test]
    fn hooks_fragment_mentions_advise_and_diff() {
        let frag = hooks_fragment();
        assert!(frag.contains("cofferdam advise"));
        assert!(frag.contains("--diff HEAD"));
        assert!(frag.contains("Edit|Write|MultiEdit"));
    }
}
