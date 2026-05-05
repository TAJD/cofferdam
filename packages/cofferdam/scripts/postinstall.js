#!/usr/bin/env node
// cofferdam postinstall — downloads the matching binary from the GitHub Release
// for this package version. Stopgap until phase 4 ships @cofferdam/node via
// napi-rs prebuilt binaries through optionalDependencies.
//
// Zero runtime deps: uses only Node stdlib (https, fs, path, crypto, child_process).
// Designed to fail loudly with actionable error messages so users know exactly
// what went wrong when corp proxies, offline installs, or sandbox environments
// block the download.
//
// Override paths:
//   - COFFERDAM_BINARY_PATH=/abs/path  → skip download, use this binary
//   - COFFERDAM_SKIP_DOWNLOAD=1        → skip postinstall entirely (CI / Docker layers)

'use strict';

const fs = require('fs');
const path = require('path');
const https = require('https');
const crypto = require('crypto');
const { execSync } = require('child_process');

const PACKAGE_DIR = path.resolve(__dirname, '..');
const PKG = require(path.join(PACKAGE_DIR, 'package.json'));
const BIN_DIR = path.join(PACKAGE_DIR, 'bin');
const VERSION = PKG.version;
const REPO = 'TAJD/cofferdam';

if (process.env.COFFERDAM_SKIP_DOWNLOAD === '1') {
  console.log('[cofferdam] COFFERDAM_SKIP_DOWNLOAD=1 set, skipping binary download.');
  process.exit(0);
}

// pnpm v10+ silently blocks postinstall scripts unless the package is in
// `pnpm.onlyBuiltDependencies` — when the user runs `pnpm add -D cofferdam`
// without that allowlist, this script never executes and the binary
// never lands. By the time we DO run (typically `pnpm rebuild cofferdam`
// after the user notices the missing binary), surface a one-line hint
// pointing at the durable fix so they don't hit the same trap on
// subsequent fresh installs (cd-iui / gh #9).
if (/^pnpm\//.test(process.env.npm_config_user_agent || '')) {
  console.log(
    '[cofferdam] pnpm detected. To make future installs work without `pnpm rebuild`,\n' +
      '            add this to your package.json:\n' +
      '              { "pnpm": { "onlyBuiltDependencies": ["cofferdam"] } }'
  );
}

if (process.env.COFFERDAM_BINARY_PATH) {
  const src = process.env.COFFERDAM_BINARY_PATH;
  if (!fs.existsSync(src)) {
    console.error(`[cofferdam] COFFERDAM_BINARY_PATH=${src} does not exist.`);
    process.exit(1);
  }
  const dest = path.join(BIN_DIR, binaryName());
  fs.mkdirSync(BIN_DIR, { recursive: true });
  fs.copyFileSync(src, dest);
  if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
  console.log(`[cofferdam] using binary from ${src}`);
  process.exit(0);
}

main().catch((err) => {
  console.error(`[cofferdam] postinstall failed: ${err.message || err}`);
  console.error('');
  console.error('Manual install fallback:');
  console.error(`  1. Download from https://github.com/${REPO}/releases/tag/v${VERSION}`);
  console.error(`  2. Extract the archive for your platform`);
  console.error(`  3. Set COFFERDAM_BINARY_PATH to the extracted binary and run \`npm rebuild cofferdam\``);
  process.exit(1);
});

async function main() {
  const target = detectTarget();
  const archive = archiveName(target);
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archive}`;
  const shaUrl = `${url}.sha256`;

  console.log(`[cofferdam] platform: ${target}`);
  console.log(`[cofferdam] downloading ${archive} ...`);

  fs.mkdirSync(BIN_DIR, { recursive: true });
  const archivePath = path.join(BIN_DIR, archive);

  try {
    await download(url, archivePath);
  } catch (err) {
    // Clean up partial download so a retry doesn't see a corrupt zero-byte file.
    if (fs.existsSync(archivePath)) {
      try { fs.unlinkSync(archivePath); } catch {}
    }
    throw err;
  }

  // SHA verification — non-fatal if .sha256 is missing (older releases),
  // fatal if present and mismatched.
  try {
    const expectedHash = (await fetchText(shaUrl)).trim().split(/\s+/)[0];
    const actualHash = sha256File(archivePath);
    if (expectedHash && expectedHash.toLowerCase() !== actualHash.toLowerCase()) {
      throw new Error(`SHA-256 mismatch: expected ${expectedHash}, got ${actualHash}`);
    }
    if (expectedHash) console.log(`[cofferdam] SHA-256 verified: ${actualHash}`);
  } catch (e) {
    if (/SHA-256 mismatch/.test(e.message)) throw e;
    console.warn(`[cofferdam] could not verify SHA-256 (${e.message}); continuing.`);
  }

  extract(archivePath, BIN_DIR);

  const binaryPath = path.join(BIN_DIR, binaryName());
  if (!fs.existsSync(binaryPath)) {
    throw new Error(`expected ${binaryPath} after extraction but did not find it`);
  }
  if (process.platform !== 'win32') fs.chmodSync(binaryPath, 0o755);
  fs.unlinkSync(archivePath);

  console.log(`[cofferdam] installed ${binaryPath}`);
}

function binaryName() {
  return process.platform === 'win32' ? 'cofferdam.exe' : 'cofferdam';
}

function detectTarget() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === 'win32' && arch === 'x64') return 'x86_64-pc-windows-msvc';
  if (platform === 'darwin' && arch === 'x64') return 'x86_64-apple-darwin';
  if (platform === 'darwin' && arch === 'arm64') return 'aarch64-apple-darwin';
  if (platform === 'linux' && arch === 'x64') return isMusl() ? 'x86_64-unknown-linux-musl' : 'x86_64-unknown-linux-gnu';
  if (platform === 'linux' && arch === 'arm64') return isMusl() ? 'aarch64-unknown-linux-musl' : 'aarch64-unknown-linux-gnu';

  throw new Error(`unsupported platform/arch combination: ${platform}/${arch}`);
}

function isMusl() {
  // Use process.report when available (Node 16+) — checks the running binary's
  // libc, not the system's. This is what we want: even on a glibc host running
  // a musl-built Node (alpine in docker), pick the musl artifact.
  try {
    const report = process.report.getReport();
    if (report.header && report.header.glibcVersionRuntime) return false;
    return true;
  } catch {
    // Fall back to ldd parsing on older Node.
    try {
      const out = execSync('ldd --version 2>&1', { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
      return /musl/i.test(out);
    } catch {
      return false; // assume gnu if we can't tell
    }
  }
}

function archiveName(target) {
  const isWin = target.includes('windows');
  return `cofferdam-v${VERSION}-${target}.${isWin ? 'zip' : 'tar.gz'}`;
}

function download(url, dest, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    const opts = {
      headers: {
        'User-Agent': `cofferdam-postinstall/${VERSION}`,
        Accept: 'application/octet-stream',
      },
    };
    https.get(url, opts, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        if (redirectsLeft <= 0) return reject(new Error('too many redirects'));
        file.close();
        fs.unlinkSync(dest);
        return resolve(download(res.headers.location, dest, redirectsLeft - 1));
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
      }
      res.pipe(file);
      file.on('finish', () => file.close(resolve));
      file.on('error', reject);
    }).on('error', reject);
  });
}

function fetchText(url, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    const opts = { headers: { 'User-Agent': `cofferdam-postinstall/${VERSION}` } };
    https.get(url, opts, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
        if (redirectsLeft <= 0) return reject(new Error('too many redirects'));
        return resolve(fetchText(res.headers.location, redirectsLeft - 1));
      }
      if (res.statusCode !== 200) return reject(new Error(`HTTP ${res.statusCode}`));
      let buf = '';
      res.setEncoding('utf8');
      res.on('data', (c) => (buf += c));
      res.on('end', () => resolve(buf));
      res.on('error', reject);
    }).on('error', reject);
  });
}

function sha256File(filePath) {
  const h = crypto.createHash('sha256');
  h.update(fs.readFileSync(filePath));
  return h.digest('hex');
}

function extract(archivePath, dest) {
  // System tar handles both .tar.gz and .zip on modern Windows / macOS / Linux.
  // We could pull tar.js as a dep but it would dominate the install footprint.
  if (archivePath.endsWith('.zip')) {
    if (process.platform === 'win32') {
      // Use PowerShell's Expand-Archive — built into every supported Windows.
      execSync(`powershell -NoProfile -Command "Expand-Archive -Force -Path '${archivePath}' -DestinationPath '${dest}'"`, {
        stdio: 'inherit',
      });
    } else {
      // On unix we'd need an unzip — should never reach here because windows is the only zip target.
      throw new Error(`zip extraction is only supported on Windows; got ${process.platform}`);
    }
  } else {
    execSync(`tar -xzf "${archivePath}" -C "${dest}"`, { stdio: 'inherit' });
  }
}
