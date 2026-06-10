# cofferdam agents

`cofferdam agents` prints a ready-to-paste markdown block that tells an AI
coding agent how to use cofferdam in the current project. The output is
version-pinned so AGENTS.md / CLAUDE.md generators can detect when it is
stale.

## Usage

```sh
cofferdam agents
```

Prints to stdout and exits 0. Append to an existing agent-context file:

```sh
cofferdam agents >> AGENTS.md
```

Or create one from scratch:

```sh
cofferdam agents > AGENTS.md
```

## What the prompt covers

- `cofferdam advise <file>` — layer membership and per-file constraints to read
  **before** editing.
- `cofferdam advise --diff <git-ref>` — pre-flight a proposed change
  (`would_fire` / `would_clear`).
- `cofferdam check --robot` — machine-readable JSON findings; `--format=compact`
  for the smallest footprint.
- `cofferdam.invariants.toml` — the single source of architectural truth.
- `cofferdam explain <Check.Id>` — full rationale for any check.
- GitHub issues URL for reporting false positives, crashes, and confusing output.
- Forward pointer to the MCP tool (cd-9r3) once it ships.

## Detecting staleness

The output header includes the cofferdam version:

```
# cofferdam agents — v0.3.6
```

When you upgrade cofferdam, re-run `cofferdam agents` and diff against the
committed copy to pick up new workflow guidance.

## Related

- [`cofferdam advise`](https://tajd.github.io/cofferdam/reference/cli/) — JIT
  per-file advisory used before edits.
- [`cofferdam check --robot`](output-formats.md) — machine-readable findings.
- [`cofferdam.invariants.toml`](invariants.md) — architectural constraints.
