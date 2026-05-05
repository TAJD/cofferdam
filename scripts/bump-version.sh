#!/usr/bin/env bash
# Bump every version string that ships in a release, in lockstep.
#
# Updates:
#   - [workspace.package].version in Cargo.toml
#   - [workspace.dependencies] path-dep version pins in Cargo.toml
#   - packages/cofferdam/package.json
#   - packages/check-sdk/package.json
#
# Single source of truth: the argument you pass in. The Rust workspace
# and the @cofferdam/* npm packages release in lockstep — see
# MAINTAINERS.md § Cutting a release for the rationale.
#
# Usage:
#   scripts/bump-version.sh 0.2.3
#
# Prereqs:
#   - cargo-edit installed: `cargo install cargo-edit --locked`
#   - npm available on PATH

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <version>" >&2
  echo "example: $0 0.2.3" >&2
  exit 2
fi

version="$1"

# Strip a leading 'v' if the caller forgot — tolerate both `0.2.3` and `v0.2.3`.
version="${version#v}"

# Validate semver-ish shape so we fail before mutating files.
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-].*)?$ ]]; then
  echo "ERROR: '$version' does not look like a semver (e.g. 0.2.3 or 0.2.3-rc1)" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

echo "==> bumping Rust workspace to $version"
# `cargo set-version --workspace` updates each member crate's version
# and rewrites matching `[workspace.dependencies]` path-dep pins in
# one call. Replaces the regex-on-TOML approach release.yml used
# pre-cd-8rp.
cargo set-version --workspace "$version"

echo "==> bumping packages/cofferdam to $version"
(cd packages/cofferdam && npm version "$version" --no-git-tag-version --allow-same-version >/dev/null)

echo "==> bumping packages/check-sdk to $version"
(cd packages/check-sdk && npm version "$version" --no-git-tag-version --allow-same-version >/dev/null)

echo
echo "Done. Review the diff, commit, and tag:"
echo
echo "  git diff -- Cargo.toml packages/"
echo "  git commit -am \"release: v$version\""
echo "  git tag v$version && git push --follow-tags"
