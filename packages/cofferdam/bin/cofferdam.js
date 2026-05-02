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
  console.error(
    `[cofferdam] binary not found at ${binaryPath}\n\n` +
      'This usually means the postinstall step did not complete. Run:\n' +
      '  npm rebuild cofferdam\n\n' +
      'If you installed with --ignore-scripts (or your CI does), the binary\n' +
      'must be installed manually. See:\n' +
      '  https://github.com/TAJD/cofferdam#install'
  );
  process.exit(1);
}

const result = spawnSync(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: false,
});

if (result.error) {
  console.error(`[cofferdam] failed to spawn binary: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status ?? 1);
