#!/usr/bin/env node
// cd-s7f — verify `cofferdam advise --diff <ref>` includes plugin findings
// in would_fire / would_clear.
//
// Sets up a self-contained git repo under a tmpdir, copies the
// no-http-client plugin (already built by an earlier CI step) plus a
// clean baseline of the fixture, commits it, then writes back the
// known-violating fixture and runs `cofferdam advise --diff HEAD`. The
// JSON output must contain Warning.NoHttpClient entries in
// would_fire — the diff is otherwise structurally identical to the
// engine-only path that cd-ugh shipped.
//
// Pre-reqs (mirrors the surrounding plugin-sdk-e2e job):
//   - target/release/cofferdam built
//   - examples-plugins/no-http-client/dist/ built
//   - examples-plugins/no-http-client/node_modules/@cofferdam/check-sdk staged

import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, copyFileSync, cpSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const REPO = resolve(import.meta.dirname, '..');
const BIN = process.env.COFFERDAM_BIN
  || join(REPO, 'target', 'release', process.platform === 'win32' ? 'cofferdam.exe' : 'cofferdam');
const PLUGIN_SRC = join(REPO, 'examples-plugins', 'no-http-client');
const FIXTURE = join(PLUGIN_SRC, 'fixture.ts');

// A clean variant of fixture.ts with no banned-import / banned-call
// usage. Picked to be small + obviously-not-in-violation rather than
// trying to derive it from the original. The original ships with
// ROVI-style violations on lines 15, 16, 21, 26, 27 (per its own
// comments) — all of those go away here.
const CLEAN_FIXTURE = `// Clean baseline for cd-s7f diff test — no banned imports/calls.
import { send } from "@org/http";

export async function fetchUser(id: string): Promise<unknown> {
  return send("/users/" + id);
}
`;

function git(cwd, args) {
  execFileSync('git', args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
}

function main() {
  const work = mkdtempSync(join(tmpdir(), 'cofferdam-plugin-diff-'));
  console.log(`workdir: ${work}`);

  // Mirror the plugin layout: dist/, node_modules/, package.json,
  // tsconfig.json, cofferdam.toml. The dist + node_modules step are
  // built earlier in the workflow; we copy verbatim.
  for (const f of ['package.json', 'tsconfig.json', 'cofferdam.toml']) {
    copyFileSync(join(PLUGIN_SRC, f), join(work, f));
  }
  cpSync(join(PLUGIN_SRC, 'dist'), join(work, 'dist'), { recursive: true });
  cpSync(join(PLUGIN_SRC, 'node_modules'), join(work, 'node_modules'), { recursive: true });
  // src/ is not strictly needed at runtime (dist/ is what cofferdam.toml
  // points at via package.json's main), but include it so the working
  // tree mirrors the source repo for diff symmetry.
  cpSync(join(PLUGIN_SRC, 'src'), join(work, 'src'), { recursive: true });

  // Baseline: clean fixture committed.
  writeFileSync(join(work, 'fixture.ts'), CLEAN_FIXTURE);
  git(work, ['init', '--quiet', '-b', 'main']);
  git(work, ['config', 'user.email', 't@example.com']);
  git(work, ['config', 'user.name', 't']);
  git(work, ['config', 'commit.gpgsign', 'false']);
  // Ignore node_modules + dist from the git history — we still want
  // them on disk for the plugin host to load.
  writeFileSync(join(work, '.gitignore'), 'node_modules\ndist\n');
  git(work, ['add', '-A']);
  git(work, ['commit', '--quiet', '--no-verify', '-m', 'clean baseline']);

  // Working-tree edit: drop in the violating fixture from the repo.
  writeFileSync(join(work, 'fixture.ts'), readFileSync(FIXTURE));

  const out = execFileSync(
    BIN,
    ['advise', '--diff', 'HEAD', '--config', './cofferdam.toml'],
    { cwd: work, encoding: 'utf8' },
  );
  const report = JSON.parse(out);

  const pluginFireCount = report.would_fire.filter(i => i.check_id === 'Warning.NoHttpClient').length;
  if (pluginFireCount < 1) {
    console.error('FAIL: expected ≥1 Warning.NoHttpClient in would_fire after clean→violating edit.');
    console.error('Got would_fire:', JSON.stringify(report.would_fire, null, 2));
    process.exit(1);
  }
  console.log(`OK: ${pluginFireCount} plugin finding(s) in would_fire.`);

  // Inverse direction: commit the violating fixture, then clear it in
  // the working tree. The plugin findings should now be in
  // would_clear, and zero plugin findings in would_fire.
  writeFileSync(join(work, 'fixture.ts'), readFileSync(FIXTURE));
  git(work, ['add', '-A']);
  git(work, ['commit', '--quiet', '--no-verify', '-m', 'introduce violations']);
  writeFileSync(join(work, 'fixture.ts'), CLEAN_FIXTURE);

  const out2 = execFileSync(
    BIN,
    ['advise', '--diff', 'HEAD', '--config', './cofferdam.toml'],
    { cwd: work, encoding: 'utf8' },
  );
  const report2 = JSON.parse(out2);
  const pluginClearCount = report2.would_clear.filter(i => i.check_id === 'Warning.NoHttpClient').length;
  if (pluginClearCount < 1) {
    console.error('FAIL: expected ≥1 Warning.NoHttpClient in would_clear after violating→clean edit.');
    console.error('Got would_clear:', JSON.stringify(report2.would_clear, null, 2));
    process.exit(1);
  }
  const stillFire = report2.would_fire.filter(i => i.check_id === 'Warning.NoHttpClient').length;
  if (stillFire !== 0) {
    console.error(`FAIL: expected 0 Warning.NoHttpClient in would_fire on clearing edit, got ${stillFire}.`);
    process.exit(1);
  }
  console.log(`OK: ${pluginClearCount} plugin finding(s) in would_clear, 0 in would_fire.`);

  console.log('');
  console.log('[OK] plugin diff coverage validated.');
}

main();
