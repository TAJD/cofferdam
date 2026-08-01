# Git hooks

Tracked, opt-in hooks for cofferdam contributors. Mirror the cheapest
`ci.yml` checks so drift gets caught locally instead of on a red CI run.

## Install (one-time, per checkout)

```bash
git config core.hooksPath .githooks
```

That's it. Git now runs `.githooks/pre-commit` on every commit.

## What runs

| Hook | Checks | Why here |
|---|---|---|
| `pre-commit` | `cargo fmt --all -- --check` + `cofferdam gen-docs --check` + `node scripts/check-vitepress.mjs` (only when `docs/*.md` changed) | Sub-second on warm cache. The checks that drift most often. `gen-docs` is the most-forgotten step during releases — see the `cut-release` skill. The VitePress check catches the three classes of docs-build failure we've shipped and reverted: bare `<Capital>` tags, literal `{{` template interpolation, and relative links escaping `docs/`. |

There is no `pre-push` hook. `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace` are enforced by CI (the `test` job, required
by the branch-protection ruleset on `main`) instead of locally — a local
pre-push run duplicated the same check a contributor (or agent) had
usually just run seconds earlier as part of their own verification, for
no independent value CI doesn't already provide as the actual merge gate.

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
