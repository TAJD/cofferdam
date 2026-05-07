# Git hooks

Tracked, opt-in hooks for cofferdam contributors. Mirror the cheapest
`ci.yml` checks so drift gets caught locally instead of on a red CI run.

## Install (one-time, per checkout)

```bash
git config core.hooksPath .githooks
```

That's it. Git now runs `.githooks/pre-commit` and `.githooks/pre-push`
on every commit / push.

## What runs

| Hook | Checks | Why here |
|---|---|---|
| `pre-commit` | `cargo fmt --all -- --check` + `cofferdam gen-docs --check` | Sub-second on warm cache. The two checks that drift most often. `gen-docs` is the most-forgotten step during releases — see the `cut-release` skill. |
| `pre-push` | `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` | 10s+ on warm cache; too slow per-commit but cheap to run before propagating to origin. Catches the same red-CI cases as the `ci` workflow. |

## Bypassing

`git commit --no-verify` / `git push --no-verify` skip the hooks. Don't
make a habit of it: the repo's `CLAUDE.md` forbids agents from using
`--no-verify`. If a hook fails, fix the underlying issue.

## Uninstalling

```bash
git config --unset core.hooksPath
```

## Why not Husky / lefthook / pre-commit?

The Rust workspace is the source of truth and `.githooks/` + a single
git config line require zero new dependencies. Adding a Node-based hook
manager would mean running `npm install` to commit Rust code.
