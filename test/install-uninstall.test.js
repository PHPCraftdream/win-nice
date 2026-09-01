'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const { install } = require('../install/install');
const { uninstall } = require('../install/uninstall');
const paths = require('../install/paths');
const manifest = require('../install/manifest');

function freshHome() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-home-'));
}

// updatePath is always false in these tests - they must never touch the real
// user PATH/registry. That mutation path is exercised manually, not here.
function withHome(home, fn) {
  const prev = process.env.WIN_NICE_HOME;
  process.env.WIN_NICE_HOME = home;
  try {
    return fn();
  } finally {
    if (prev === undefined) delete process.env.WIN_NICE_HOME;
    else process.env.WIN_NICE_HOME = prev;
    fs.rmSync(home, { recursive: true, force: true });
  }
}

test('install copies every bin/*.bat and *.ps1 file and writes a manifest', () => {
  withHome(freshHome(), () => {
    const result = install({ updatePath: false });
    assert.ok(result, 'install should not skip when WIN_NICE_HOME is set');
    const dir = paths.binDir();
    const files = fs.readdirSync(dir).sort();
    assert.deepEqual(files, result.files.slice().sort());
    assert.ok(files.includes('cap.ps1'));
    assert.ok(files.includes('idle.bat'));

    const data = manifest.read(paths.manifestPath());
    assert.ok(data);
    assert.equal(data.binDir, dir);
    assert.deepEqual(data.files.slice().sort(), files);
  });
});

test('uninstall removes every file listed in the manifest and the manifest itself', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(dir), false);
    assert.equal(fs.existsSync(paths.manifestPath()), false);
  });
});

test('uninstall skips a file whose marker was stripped (user-modified) instead of deleting it', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const target = path.join(dir, 'idle.bat');
    fs.writeFileSync(target, '@echo off\r\necho user replaced this file\r\n');

    const results = uninstall({ updatePath: false });
    const idleResult = results.find((r) => r.file === target);
    assert.equal(idleResult.removed, false);
    assert.equal(fs.existsSync(target), true, 'modified file must survive uninstall');
  });
});

test('uninstall falls back to scanning binDir for marked files when the manifest is gone', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    fs.unlinkSync(paths.manifestPath());

    const dir = paths.binDir();
    assert.ok(fs.readdirSync(dir).length > 0);

    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(dir), false);
  });
});

test('uninstall fallback scan never removes an unmarked file dropped into binDir', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    fs.unlinkSync(paths.manifestPath());
    const dir = paths.binDir();
    const foreign = path.join(dir, 'not-ours.txt');
    fs.writeFileSync(foreign, 'unrelated user file');

    uninstall({ updatePath: false });
    assert.equal(fs.existsSync(foreign), true, 'unmarked foreign file must survive');
  });
});

test('reinstall (uninstall + install) leaves a clean, fully populated bin dir', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    uninstall({ updatePath: false });
    const result = install({ updatePath: false });
    const dir = paths.binDir();
    assert.deepEqual(fs.readdirSync(dir).sort(), result.files.slice().sort());
  });
});

test('install is a no-op guard when run from the source checkout without WIN_NICE_HOME', () => {
  const prev = process.env.WIN_NICE_HOME;
  delete process.env.WIN_NICE_HOME;
  try {
    const result = install();
    assert.equal(result, null);
  } finally {
    if (prev !== undefined) process.env.WIN_NICE_HOME = prev;
  }
});
