#!/usr/bin/env node
// Tiny shim: forward all argv + stdio to the platform-specific binary that
// postinstall placed alongside this script.

'use strict';

const { spawnSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const binaryName = process.platform === 'win32' ? 'cofferdam.exe' : 'cofferdam';
const binaryPath = path.join(__dirname, binaryName);

if (!fs.existsSync(binaryPath)) {
  // Tailor the recovery hint to the package manager. pnpm v10's default
  // sandbox silently blocks postinstall unless the package is in
  // `pnpm.onlyBuiltDependencies`, so a `pnpm add -D cofferdam` 'succeeds'
  // with no binary on disk. Recommending `npm rebuild` to those users is
  // wrong — they need pnpm syntax + the allowlist edit (cd-iui / gh #9).
  const pkgManager = detectPackageManager();
  let recovery;
  if (pkgManager === 'pnpm') {
    recovery =
      "Looks like a pnpm install. pnpm v10 blocks postinstall scripts unless\n" +
      "the package is in `pnpm.onlyBuiltDependencies`. To fix:\n\n" +
      '  1. Add to your package.json:\n' +
      '       { "pnpm": { "onlyBuiltDependencies": ["cofferdam"] } }\n' +
      '  2. Then run:  pnpm rebuild cofferdam\n';
  } else if (pkgManager === 'yarn') {
    recovery =
      'Looks like a yarn install. Run:\n' +
      '  yarn rebuild cofferdam\n';
  } else {
    recovery = 'Run:\n  npm rebuild cofferdam\n';
  }

  console.error(
    `[cofferdam] binary not found at ${binaryPath}\n\n` +
      'This usually means the postinstall step did not complete.\n\n' +
      recovery + '\n' +
      'If you installed with --ignore-scripts (or your CI does), the binary\n' +
      'must be installed manually. See:\n' +
      '  https://github.com/TAJD/cofferdam#install'
  );
  process.exit(1);
}

// Cheap heuristic: walk up from this script's location looking for the
// canonical lockfile / metadata each package manager leaves behind. Falls
// back to `process.env.npm_config_user_agent` when we're inside a fresh
// shell invocation that hasn't touched node_modules. Returns one of
// 'pnpm' | 'yarn' | 'npm' | null.
function detectPackageManager() {
  const ua = process.env.npm_config_user_agent || '';
  if (/^pnpm\//.test(ua)) return 'pnpm';
  if (/^yarn\//.test(ua)) return 'yarn';
  if (/^npm\//.test(ua)) return 'npm';

  // Walk up from __dirname looking for tell-tale files. Most reliable:
  // an adjacent `node_modules/.pnpm/` (pnpm's content-addressed store).
  let dir = path.resolve(__dirname);
  for (let i = 0; i < 20; i++) {
    if (fs.existsSync(path.join(dir, 'node_modules', '.pnpm'))) return 'pnpm';
    if (fs.existsSync(path.join(dir, 'pnpm-lock.yaml'))) return 'pnpm';
    if (fs.existsSync(path.join(dir, 'yarn.lock'))) return 'yarn';
    if (fs.existsSync(path.join(dir, 'package-lock.json'))) return 'npm';
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return null;
}

// Guard against broken-pipe conditions (e.g. `cofferdam check --format=json | head`).
//
// On POSIX, when the consumer closes the pipe, the kernel sends SIGPIPE to the
// child process first (Rust's stdlib already handles it cleanly). The pnpm
// wrapper — this script — may still have its own writes fail with EPIPE (e.g.
// the error-not-found message path above, or any Node internal flush). Node 22+
// delivers SIGPIPE to non-default handlers; older versions silently terminate.
// Belt-and-suspenders: listen for both the signal and the stream error.
//
// On Windows there is no SIGPIPE signal; writes to a closed pipe throw EPIPE
// synchronously through the stream 'error' event. The error handlers cover that
// path — registering the signal handler on Windows is harmless (never fires).
process.on('SIGPIPE', () => process.exit(0));
process.stdout.on('error', (err) => { if (err.code === 'EPIPE') process.exit(0); else throw err; });
process.stderr.on('error', (err) => { if (err.code === 'EPIPE') process.exit(0); else throw err; });

const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
});

if (result.error) {
  console.error(`[cofferdam] failed to spawn binary: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
