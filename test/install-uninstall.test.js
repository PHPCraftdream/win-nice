'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const crypto = require('node:crypto');

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
    assert.ok(files.includes('capc.ps1'));
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

test('uninstall removes a manifest-tracked file even if its marker was stripped (user-modified)', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const target = path.join(dir, 'idle.bat');
    fs.writeFileSync(target, '@echo off\r\necho user replaced this file\r\n');

    const results = uninstall({ updatePath: false });
    const idleResult = results.find((r) => r.file === target);
    assert.equal(idleResult.removed, true);
    assert.equal(fs.existsSync(target), false, 'manifest-tracked files are owned by the package');
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

test('uninstall rejects a manifest entry that traverses outside the install directory', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const dir = paths.binDir();
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);

    const outsideTarget = path.join(dir, '..', 'outside.txt');
    fs.writeFileSync(outsideTarget, 'must survive');
    data.files.push('../outside.txt');
    manifest.write(manifestFile, data);

    const results = uninstall({ updatePath: false });
    assert.equal(fs.existsSync(outsideTarget), true, 'traversal target must survive uninstall');
    const rejected = results.find((r) => path.resolve(r.file) === path.resolve(outsideTarget));
    assert.equal(rejected.removed, false);
    fs.rmSync(outsideTarget, { force: true });
  });
});

test('uninstall rejects a manifest entry that is an absolute path outside the install directory', () => {
  withHome(freshHome(), () => {
    install({ updatePath: false });
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);

    const outsideTarget = fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-abs-'));
    const outsideFile = path.join(outsideTarget, 'evil.txt');
    fs.writeFileSync(outsideFile, 'must survive');
    data.files.push(outsideFile);
    manifest.write(manifestFile, data);

    const results = uninstall({ updatePath: false });
    assert.equal(fs.existsSync(outsideFile), true, 'absolute-path target must survive uninstall');
    const rejected = results.find((r) => r.reason === 'rejected (escapes install directory)');
    assert.ok(rejected, 'the absolute-path entry must be explicitly rejected, not just missed');
    fs.rmSync(outsideTarget, { recursive: true, force: true });
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

test('install (upgrade path) removes a stale manifest-tracked file the current version no longer ships (e.g. a renamed tool)', () => {
  withHome(freshHome(), () => {
    const first = install({ updatePath: false });
    const dir = paths.binDir();

    // Simulate what a previous version would have left behind: a tool that
    // doesn't exist in the current bin/ (e.g. before a rename/removal).
    const staleNames = ['oldtool.bat', 'oldtool.ps1', 'oldtool'];
    for (const name of staleNames) fs.writeFileSync(path.join(dir, name), 'stale');
    const manifestFile = paths.manifestPath();
    const data = manifest.read(manifestFile);
    data.files = data.files.concat(staleNames);
    manifest.write(manifestFile, data);

    // `npm install -g win-nice@newer` only runs install() (postinstall) - it
    // never calls uninstall() first, unlike `win-nice reinstall`. This has to
    // be caught by cleanupStaleFiles() inside install() itself.
    const result = install({ updatePath: false });

    for (const name of staleNames) {
      assert.equal(fs.existsSync(path.join(dir, name)), false, `${name} (stale, no longer shipped) must be removed on upgrade`);
    }
    for (const name of first.files) {
      assert.equal(fs.existsSync(path.join(dir, name)), true, `${name} (still shipped) must survive`);
    }
    assert.deepEqual(
      manifest.read(manifestFile).files.slice().sort(),
      result.files.slice().sort(),
      'manifest must not still list the stale names'
    );
  });
});

test('install is a no-op guard when run from the source checkout without WIN_NICE_HOME', () => {
  const prev = process.env.WIN_NICE_HOME;
  delete process.env.WIN_NICE_HOME;
  try {
    // updatePath: false - if this guard ever regresses, the test must not fall
    // through to a real PATH read/write against the developer's actual registry.
    const result = install({ updatePath: false });
    assert.equal(result, null);
  } finally {
    if (prev !== undefined) process.env.WIN_NICE_HOME = prev;
  }
});

// PID alone can be reused across runs/reboots, so a crashed run could leave a
// colliding scratch key behind; the GUID makes that impossible.
function scratchKey() {
  return `HKCU:\\Software\\WinNiceTest\\${process.pid}-${crypto.randomUUID()}`;
}

test('readRegistryString/writeRegistryString round-trip non-ASCII values exactly, on a scratch key', () => {
  const keyPath = scratchKey();
  const valueName = 'ScratchPath';
  const value = 'C:\\Users\\Марат\\bin;C:\\Users\\José\\bin';
  try {
    paths.writeRegistryString(keyPath, valueName, value);
    const readBack = paths.readRegistryString(keyPath, valueName);
    assert.equal(readBack, value);
  } finally {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      'Remove-Item -LiteralPath $env:WIN_NICE_REG_KEY -Recurse -Force -ErrorAction SilentlyContinue',
    ], { env: { ...process.env, WIN_NICE_REG_KEY: keyPath } });
  }
});

test('writeRegistryString preserves REG_EXPAND_SZ across a round trip instead of flattening it', () => {
  const keyPath = scratchKey();
  const valueName = 'ScratchExpand';
  try {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      [
        'if (-not (Test-Path -LiteralPath $env:WIN_NICE_REG_KEY)) { New-Item -Path $env:WIN_NICE_REG_KEY -Force | Out-Null }',
        'Set-ItemProperty -LiteralPath $env:WIN_NICE_REG_KEY -Name $env:WIN_NICE_REG_VALUE -Value $env:WIN_NICE_REG_NEW_VALUE -Type ExpandString',
      ].join('\n'),
    ], {
      env: { ...process.env, WIN_NICE_REG_KEY: keyPath, WIN_NICE_REG_VALUE: valueName, WIN_NICE_REG_NEW_VALUE: '%USERPROFILE%\\bin' },
    });

    // Round-trip through writeRegistryString, as install()/uninstall() do.
    paths.writeRegistryString(keyPath, valueName, '%USERPROFILE%\\bin2;C:\\extra');

    const kind = execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      '(Get-Item -LiteralPath $env:WIN_NICE_REG_KEY).GetValueKind($env:WIN_NICE_REG_VALUE)',
    ], { encoding: 'utf8', env: { ...process.env, WIN_NICE_REG_KEY: keyPath, WIN_NICE_REG_VALUE: valueName } });
    assert.equal(kind.trim(), 'ExpandString');

    const raw = paths.readRegistryString(keyPath, valueName);
    assert.equal(raw, '%USERPROFILE%\\bin2;C:\\extra', 'value must stay unexpanded');
  } finally {
    execFileSync('powershell', [
      '-NoProfile',
      '-Command',
      'Remove-Item -LiteralPath $env:WIN_NICE_REG_KEY -Recurse -Force -ErrorAction SilentlyContinue',
    ], { env: { ...process.env, WIN_NICE_REG_KEY: keyPath } });
  }
});

test('addToPathString/removeFromPathString treat a raw %LOCALAPPDATA% entry as its expanded equivalent', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%LOCALAPPDATA%\\win-nice\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw, 'add must dedup against the raw equivalent, not append a duplicate');
  assert.equal(paths.removeFromPathString(raw, expanded), '', 'remove must strip the raw equivalent entry');
});

test('addToPathString/removeFromPathString treat a raw %USERPROFILE% entry as its expanded equivalent', () => {
  const expanded = path.join(process.env.USERPROFILE, 'somewhere', 'bin');
  const raw = '%USERPROFILE%\\somewhere\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw);
  assert.equal(paths.removeFromPathString(raw, expanded), '');
});

test('%VAR% name matching is case-insensitive (%localappdata% matches %LOCALAPPDATA%)', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%localappdata%\\win-nice\\bin';
  assert.equal(paths.addToPathString(raw, expanded), raw);
  assert.equal(paths.removeFromPathString(raw, expanded), '');
});

test('unknown %VAR% stays literal text and never matches an unrelated directory', () => {
  const expanded = path.join(process.env.LOCALAPPDATA, 'win-nice', 'bin');
  const raw = '%NOT_A_REAL_VAR%\\win-nice\\bin';
  const added = paths.addToPathString(raw, expanded);
  assert.equal(added, `${raw};${expanded}`, 'unexpandable entry must not dedup against the expanded dir');
  assert.equal(paths.removeFromPathString(added, expanded), raw, 'removal must drop only the expanded entry and keep the literal one');
  assert.equal(paths.removeFromPathString(raw, path.join('C:', 'nope', 'bin')), raw, 'literal entry survives removal of a different directory');
});
