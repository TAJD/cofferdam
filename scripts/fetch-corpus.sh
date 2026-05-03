#!/usr/bin/env bash
# fetch-corpus.sh — clone a curated set of real-world TypeScript repos at
# pinned stable tags into tests/corpus/ (gitignored, on-demand).
#
# Repos selected (2026-05-03, verified via git ls-remote --tags):
#   colinhacks/zod  v4.4.2  — generics + conditional types stress test
#   vitejs/vite     v8.0.10 — plugin generics, large monorepo
#   nestjs/nest     v11.1.19 — decorators at scale
#
# Usage: bash scripts/fetch-corpus.sh
#   Re-running is idempotent: already-cloned repos at the expected tag are skipped.
#   One failed clone does not abort the loop; exit code is non-zero if any failed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CORPUS_DIR="$REPO_ROOT/tests/corpus"

# ---------------------------------------------------------------------------
# Corpus definition — parallel arrays (bash 3 compatible)
# ---------------------------------------------------------------------------
REPO_URLS=(
    "https://github.com/colinhacks/zod.git"
    "https://github.com/vitejs/vite.git"
    "https://github.com/nestjs/nest.git"
)
REPO_TAGS=(
    "v4.4.2"
    "v8.0.10"
    "v11.1.19"
)
REPO_DESTS=(
    "zod"
    "vite"
    "nest"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf '\033[0;34m[corpus]\033[0m %s\n' "$*"; }
ok()    { printf '\033[0;32m[corpus]\033[0m %s\n' "$*"; }
warn()  { printf '\033[0;33m[corpus]\033[0m %s\n' "$*" >&2; }
err()   { printf '\033[0;31m[corpus]\033[0m %s\n' "$*" >&2; }

# Check whether an already-present clone is at the expected tag.
# Returns 0 if it is (skip clone), 1 if the tag doesn't match or can't be read.
is_at_tag() {
    local dest="$1" expected_tag="$2"
    local git_dir="$CORPUS_DIR/$dest/.git"
    if [[ ! -f "$git_dir/HEAD" ]]; then
        return 1
    fi
    # In a shallow clone the tag ref is stored under packed-refs or refs/tags/
    local packed="$git_dir/packed-refs"
    local tag_ref="refs/tags/$expected_tag"
    if [[ -f "$packed" ]] && grep -qF "$tag_ref" "$packed"; then
        return 0
    fi
    if [[ -f "$git_dir/$tag_ref" ]]; then
        return 0
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
mkdir -p "$CORPUS_DIR"

total=${#REPO_URLS[@]}
fetched=0
failed=0

for i in "${!REPO_URLS[@]}"; do
    url="${REPO_URLS[$i]}"
    tag="${REPO_TAGS[$i]}"
    dest="${REPO_DESTS[$i]}"
    dest_path="$CORPUS_DIR/$dest"

    if is_at_tag "$dest" "$tag"; then
        ok "already present: $dest @ $tag (skip)"
        (( fetched++ )) || true
        continue
    fi

    # If the directory exists but is at the wrong tag, remove it first.
    if [[ -d "$dest_path" ]]; then
        warn "stale clone at $dest — removing and re-cloning"
        rm -rf "$dest_path"
    fi

    info "cloning $url @ $tag → tests/corpus/$dest"
    # Disable pipefail for the clone so one failure doesn't exit the script.
    set +e
    git clone --depth 1 --branch "$tag" "$url" "$dest_path" 2>&1
    clone_exit=$?
    set -e

    if [[ $clone_exit -ne 0 ]]; then
        err "FAILED: $dest @ $tag (exit $clone_exit) — continuing"
        (( failed++ )) || true
        # Remove partial clone directory if present.
        rm -rf "$dest_path"
    else
        ok "fetched: $dest @ $tag"
        (( fetched++ )) || true
    fi
done

echo ""
printf 'Fetched %d/%d corpus repos' "$fetched" "$total"
if [[ $failed -gt 0 ]]; then
    printf ' (%d failed)\n' "$failed"
    exit 1
fi
printf '\n'
