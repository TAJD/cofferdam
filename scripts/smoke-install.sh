#!/usr/bin/env bash
# Hermetic install smoke test for the `cofferdam` npm package (cd-u58).
#
# Flow mirrors what a real `npm install cofferdam` does, except the
# postinstall is short-circuited via COFFERDAM_BINARY_PATH so we exercise
# the package wiring (npm pack contents, postinstall path resolution,
# bin shim spawn) without round-tripping through GitHub Releases. The
# release-tarball path is covered separately by the `npm-smoke` job in
# release.yml that runs against the just-published version.
#
#   1. cd packages/cofferdam && npm pack
#   2. install the .tgz into a tmpdir with COFFERDAM_BINARY_PATH pointing
#      at target/release/cofferdam[.exe]
#   3. npx cofferdam check examples/triple_equals.ts --format json
#   4. assert exit code (0 or 1 — findings produce exit 1), JSON contains
#      a Warning.TripleEquals issue, and node_modules/cofferdam/bin/<bin>
#      exists.
#
# Local repro:
#   cargo build --release --bin cofferdam
#   ./scripts/smoke-install.sh
#
# Override the binary location:
#   COFFERDAM_BINARY_PATH=/abs/path/cofferdam ./scripts/smoke-install.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) BIN_NAME=cofferdam.exe ;;
  *) BIN_NAME=cofferdam ;;
esac

BINARY="${COFFERDAM_BINARY_PATH:-$REPO_ROOT/target/release/$BIN_NAME}"

if [[ ! -f "$BINARY" ]]; then
  echo "ERROR: cofferdam binary not found at $BINARY" >&2
  echo "Build it first:  cargo build --release --bin cofferdam" >&2
  echo "Or set COFFERDAM_BINARY_PATH to point at an existing binary." >&2
  exit 1
fi

PKG_DIR="$REPO_ROOT/packages/cofferdam"
cd "$PKG_DIR"

echo "==> npm pack ($PKG_DIR)"
# `npm pack --json` returns a JSON array; the .filename is the tgz name.
PACK_JSON=$(npm pack --json --silent)
TARBALL=$(node -e '
  const d = JSON.parse(require("fs").readFileSync(0, "utf8"));
  process.stdout.write(d[0].filename);
' <<< "$PACK_JSON")
TARBALL_ABS="$PKG_DIR/$TARBALL"

WORKDIR=$(mktemp -d 2>/dev/null || mktemp -d -t 'cofferdam-smoke')
cleanup() {
  rm -f "$TARBALL_ABS"
  # node_modules on Windows can hold locks briefly after npm exits.
  rm -rf "$WORKDIR" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> workdir: $WORKDIR"
cd "$WORKDIR"
npm init -y >/dev/null

echo "==> npm install (COFFERDAM_BINARY_PATH=$BINARY)"
COFFERDAM_BINARY_PATH="$BINARY" \
  npm install --no-audit --no-fund --foreground-scripts "$TARBALL_ABS"

INSTALLED_BIN="$WORKDIR/node_modules/cofferdam/bin/$BIN_NAME"
if [[ ! -f "$INSTALLED_BIN" ]]; then
  echo "FAIL: binary not at $INSTALLED_BIN" >&2
  ls -la "$WORKDIR/node_modules/cofferdam/bin/" >&2 || true
  exit 1
fi
echo "OK: binary present at $INSTALLED_BIN"

FIXTURE="$REPO_ROOT/examples/triple_equals.ts"
echo "==> npx cofferdam check $FIXTURE --format json"
set +e
OUT=$(npx --no-install cofferdam check "$FIXTURE" --format json)
EC=$?
set -e

# Findings present → exit 1 (default --fail-on=medium and Warning.TripleEquals
# clears the bar). 0 would mean no findings, which is itself a fail for this
# fixture. >1 is a binary error.
if [[ "$EC" -ne 0 && "$EC" -ne 1 ]]; then
  echo "FAIL: unexpected exit code $EC from cofferdam check" >&2
  echo "--- stdout ---" >&2
  echo "$OUT" >&2
  exit 1
fi

# Validate JSON shape and assert TripleEquals is in there.
node -e '
  const data = process.argv[1];
  let r;
  try { r = JSON.parse(data); }
  catch (e) {
    console.error("FAIL: stdout was not valid JSON: " + e.message);
    console.error("--- raw stdout ---");
    console.error(data);
    process.exit(1);
  }
  if (!Array.isArray(r.findings)) {
    console.error("FAIL: top-level .findings is not an array");
    console.error(JSON.stringify(r, null, 2));
    process.exit(1);
  }
  if (r.findings.length === 0) {
    console.error("FAIL: zero findings against examples/triple_equals.ts");
    process.exit(1);
  }
  const triple = r.findings.find(f => f.id === "Warning.TripleEquals");
  if (!triple) {
    const ids = r.findings.map(f => f.id).join(", ");
    console.error("FAIL: Warning.TripleEquals not in findings (got: " + ids + ")");
    process.exit(1);
  }
  console.log("OK: Warning.TripleEquals at " + (triple.file || "?") + ":" + (triple.line || "?"));
' "$OUT"

echo
echo "[OK] smoke install passed (binary=$BINARY)"
