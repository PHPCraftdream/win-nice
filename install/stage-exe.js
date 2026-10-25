#!/usr/bin/env node
'use strict';
// Copies the freshly built Rust exe into bin/ (what npm packs) or, with
// --check, just gates `npm pack` on it existing. Run from anywhere inside
// the repo.
const fs = require('fs');
const path = require('path');

function repoRoot() {
  return path.join(__dirname, '..');
}

function assertStagedExe() {
  const target = path.join(repoRoot(), 'bin', 'wnice.exe');
  if (fs.existsSync(target)) return target;
  console.error('prepack: bin/wnice.exe is missing - run "npm run build:exe" (requires a Rust toolchain) or use a CI-provided binary');
  process.exitCode = 1;
  return null;
}

function stage() {
  const source = path.join(repoRoot(), 'target', 'release', 'wnice.exe');
  if (!fs.existsSync(source)) {
    console.error(`missing ${source} - run "cargo build --release --locked" first (or "npm run build:exe")`);
    process.exitCode = 1;
    return;
  }
  fs.mkdirSync(path.join(repoRoot(), 'bin'), { recursive: true });
  fs.copyFileSync(source, path.join(repoRoot(), 'bin', 'wnice.exe'));
  console.log('staged bin/wnice.exe');
}

function main() {
  if (process.argv.includes('--check')) {
    assertStagedExe();
  } else {
    stage();
  }
}

if (require.main === module) main();

module.exports = { assertStagedExe, stage };
