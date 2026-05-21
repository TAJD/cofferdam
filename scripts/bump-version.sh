#!/usr/bin/env bash
# Thin wrapper around the canonical deterministic version manager,
# scripts/version.mjs (cd-9hp.2). Kept for muscle-memory / older docs.
#
# version.mjs supersedes the old `cargo set-version` + `npm version`
# approach: it is self-contained (no cargo-edit dependency), bumps every
# cofferdam-* path-dep pin across ALL crate Cargo.toml files (the old
# approach missed the cofferdam-cli -> cofferdam-lsp pin), and also
# regenerates Cargo.lock + docs/public/checks.json with --regen.
#
# Usage:
#   scripts/bump-version.sh 0.3.4
#
# Prereqs: node on PATH.

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>" >&2
  echo "example: $0 0.3.4" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1#v}"

node "$repo_root/scripts/version.mjs" set "$version" --regen

echo
echo "Verify, then commit + tag:"
echo "  node scripts/version.mjs check $version"
echo "  git commit -am \"release: v$version\""
echo "  git tag -a v$version -m \"v$version — ...\" && git push --follow-tags"
