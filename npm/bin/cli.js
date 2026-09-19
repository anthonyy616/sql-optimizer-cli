#!/usr/bin/env node
'use strict';

// npm launcher for sql-optimizer-cli.
//
// npm installs the platform-specific package (e.g. sql-optimizer-cli-darwin-arm64)
// via optionalDependencies; this script finds the prebuilt Rust binary inside it
// and spawns it with all arguments, passing stdin/stdout/stderr through so the
// full-screen TUI works exactly like the native binary.

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

const PLATFORM_PACKAGES = {
  'darwin-arm64': 'sql-optimizer-cli-darwin-arm64',
  'darwin-x64': 'sql-optimizer-cli-darwin-x64',
  'linux-x64': 'sql-optimizer-cli-linux-x64',
  'linux-arm64': 'sql-optimizer-cli-linux-arm64',
};

function findBinary() {
  const platformKey = `${process.platform}-${process.arch}`;
  const pkgName = PLATFORM_PACKAGES[platformKey];

  if (!pkgName) {
    console.error(
      `sql-optimizer-cli: no prebuilt binary for ${platformKey}.\n` +
        'Supported: darwin-arm64, darwin-x64, linux-x64, linux-arm64.\n' +
        'Alternatively install from source: cargo install sql-optimizer-cli'
    );
    process.exit(1);
  }

  try {
    const pkgJsonPath = require.resolve(`${pkgName}/package.json`);
    const binaryPath = path.join(path.dirname(pkgJsonPath), 'sql-optimizer-cli');

    fs.accessSync(binaryPath, fs.constants.X_OK);
    return binaryPath;
  } catch (err) {
    console.error(
      `sql-optimizer-cli: could not find the prebuilt binary (${pkgName}).\n` +
        `Reason: ${err.message}\n` +
        'Fixes: reinstall with `npm install -g sql-optimizer-cli`, or check\n' +
        'that your npm registry is not filtering optional dependencies\n' +
        '(npm config get omit — "optional" must not be set).'
    );
    process.exit(1);
  }
}

const binaryPath = findBinary();

// stdio: 'inherit' keeps the TTY attached — required for the TUI's raw-mode
// rendering and for colored output elsewhere.
const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: 'inherit',
  env: process.env,
});

for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => {
    if (child.pid) child.kill(signal);
  });
}

child.on('error', (err) => {
  console.error(`sql-optimizer-cli: failed to launch binary: ${err.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  // Mirror the binary's exit codes (0 clean / 1 warnings / 2 blocking / 3 error).
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
