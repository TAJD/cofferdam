#!/usr/bin/env node
// End-to-end npm install smoke test for the `@cofferdam/cofferdam` package.
//
// Spawned by .github/workflows/release.yml's `npm-smoke` job after
// `publish-npm` lands a new version. Verifies, on a fresh machine,
// that the user-facing install story actually works:
//
//   1. `npm install -D @cofferdam/cofferdam@<version>` puts a working
//      binary under node_modules/@cofferdam/cofferdam/bin (postinstall ran).
//   2. `npx cofferdam check src --robot` exits 1 and the JSON report
//      includes a `Design.ReadonlyArrayParam` finding (proves the binary
//      runs end-to-end on this OS+arch).
//   3. Re-installing with `--ignore-scripts` skips postinstall, the
//      binary is absent, and the JS shim exits 1 with the
//      manual-install instructions printed to stderr.
//
// Usage: node .github/scripts/npm-smoke.mjs <version>
//
// Pure Node, no third-party deps. Works under bash on Linux/macOS and
// under bash (via Git Bash) on the Windows runner.

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const version = process.argv[2];
if (!version) {
  console.error('usage: node npm-smoke.mjs <version>');
  process.exit(2);
}

const root = mkdtempSync(join(tmpdir(), 'cofferdam-smoke-'));
console.log(`tmp dir: ${root}`);

function run(label, cmd, args, opts = {}) {
  console.log(`\n--- ${label} ---`);
  // shell:true so npm's .cmd shim resolves on Windows runners.
  const result = spawnSync(cmd, args, {
    stdio: ['inherit', 'pipe', 'pipe'],
    encoding: 'utf8',
    shell: true,
    ...opts,
  });
  process.stdout.write(result.stdout ?? '');
  process.stderr.write(result.stderr ?? '');
  return result;
}

function fail(msg) {
  console.error(`\nFAIL: ${msg}`);
  process.exit(1);
}

const isWindows = process.platform === 'win32';
const binName = isWindows ? 'cofferdam.exe' : 'cofferdam';

// --- happy path ---
const happy = join(root, 'happy');
mkdirSync(happy, { recursive: true });
const init = run('npm init -y (happy)', 'npm', ['init', '-y'], { cwd: happy });
if (init.status !== 0) fail(`npm init exited ${init.status}`);

const install = run(
  'npm install -D @cofferdam/cofferdam',
  'npm',
  ['install', '-D', `@cofferdam/cofferdam@${version}`],
  { cwd: happy },
);
if (install.status !== 0) fail(`npm install exited ${install.status}`);

const binPath = join(happy, 'node_modules', '@cofferdam', 'cofferdam', 'bin', binName);
if (!existsSync(binPath)) fail(`binary not at ${binPath} after postinstall`);
console.log(`OK: binary present at ${binPath}`);

const srcDir = join(happy, 'src');
mkdirSync(srcDir, { recursive: true });
writeFileSync(
  join(srcDir, 'index.ts'),
  'export function total(items: number[]): number {\n  return items.reduce((sum, n) => sum + n, 0);\n}\n',
);
writeFileSync(
  join(happy, 'tsconfig.json'),
  '{"compilerOptions":{"target":"es2020","strict":true},"include":["src"]}\n',
);

const check = run(
  'npx cofferdam check src --robot',
  'npx',
  ['cofferdam', 'check', 'src', '--robot'],
  { cwd: happy },
);
if (check.status !== 1) {
  fail(`expected exit 1 from check, got ${check.status}`);
}

let report;
try {
  report = JSON.parse(check.stdout ?? '');
} catch (e) {
  fail(`stdout was not valid JSON: ${e.message}`);
}

if (!Array.isArray(report.findings) || report.findings.length === 0) {
  fail(`no findings in JSON report`);
}
const readonlyArrayParam = report.findings.find((f) => f.id === 'Design.ReadonlyArrayParam');
if (!readonlyArrayParam) {
  const ids = report.findings.map((f) => f.id).join(', ');
  fail(`expected Design.ReadonlyArrayParam in findings, got: ${ids}`);
}
console.log(`OK: Design.ReadonlyArrayParam fired at ${readonlyArrayParam.file}:${readonlyArrayParam.line}`);

// --- --ignore-scripts path ---
const noScripts = join(root, 'noscripts');
mkdirSync(noScripts, { recursive: true });
const initNs = run('npm init -y (noscripts)', 'npm', ['init', '-y'], { cwd: noScripts });
if (initNs.status !== 0) fail(`npm init exited ${initNs.status}`);

const installNs = run(
  'npm install -D --ignore-scripts',
  'npm',
  ['install', '-D', '--ignore-scripts', `@cofferdam/cofferdam@${version}`],
  { cwd: noScripts },
);
if (installNs.status !== 0) fail(`npm install --ignore-scripts exited ${installNs.status}`);

const noScriptsBin = join(noScripts, 'node_modules', '@cofferdam', 'cofferdam', 'bin', binName);
if (existsSync(noScriptsBin)) {
  fail(`--ignore-scripts should NOT have placed binary at ${noScriptsBin}`);
}
console.log('OK: binary absent under --ignore-scripts as expected');

const shim = run(
  'npx cofferdam --version (shim should fail)',
  'npx',
  ['cofferdam', '--version'],
  { cwd: noScripts },
);
if (shim.status === 0) {
  fail(`shim should exit non-zero, got 0`);
}
const stderr = shim.stderr ?? '';
if (!stderr.includes('binary not found') || !stderr.includes('postinstall')) {
  fail(`shim error message missing expected guidance:\n${stderr}`);
}
console.log('OK: shim printed manual-install guidance');

// Cleanup is best-effort — Windows occasionally locks node_modules.
try {
  rmSync(root, { recursive: true, force: true });
} catch {
  /* ignore */
}

console.log('\n✓ npm install smoke test passed');
