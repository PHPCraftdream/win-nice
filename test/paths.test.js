'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const paths = require('../install/paths');

test('addToPathString appends when missing', () => {
  const result = paths.addToPathString('C:\\a;C:\\b', 'C:\\c');
  assert.equal(result, 'C:\\a;C:\\b;C:\\c');
});

test('addToPathString is a no-op when already present (case-insensitive, trailing-slash tolerant)', () => {
  const result = paths.addToPathString('C:\\a;C:\\B\\', 'c:\\b');
  assert.equal(result, 'C:\\a;C:\\B\\');
});

test('addToPathString on an empty PATH', () => {
  const result = paths.addToPathString('', 'C:\\c');
  assert.equal(result, 'C:\\c');
});

test('removeFromPathString drops the matching entry', () => {
  const result = paths.removeFromPathString('C:\\a;C:\\b;C:\\c', 'C:\\b');
  assert.equal(result, 'C:\\a;C:\\c');
});

test('removeFromPathString is a no-op when absent', () => {
  const result = paths.removeFromPathString('C:\\a;C:\\c', 'C:\\b');
  assert.equal(result, 'C:\\a;C:\\c');
});

test('removeFromPathString drops duplicate matching entries too', () => {
  const result = paths.removeFromPathString('C:\\a;C:\\b;C:\\b;C:\\c', 'C:\\b');
  assert.equal(result, 'C:\\a;C:\\c');
});

test('binDir/manifestPath honor the WIN_NICE_HOME override', () => {
  const prev = process.env.WIN_NICE_HOME;
  process.env.WIN_NICE_HOME = 'C:\\scratch';
  try {
    assert.equal(paths.binDir(), path.join('C:\\scratch', 'bin'));
    assert.equal(paths.manifestPath(), path.join('C:\\scratch', 'install-manifest.json'));
  } finally {
    if (prev === undefined) delete process.env.WIN_NICE_HOME;
    else process.env.WIN_NICE_HOME = prev;
  }
});

test('powershellPath resolves Windows PowerShell from an absolute system path', () => {
  const systemRoot = 'D:\\Windows';
  assert.equal(
    paths.powershellPath({ SystemRoot: systemRoot }),
    path.win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe')
  );
});

test('powershellPath rejects a relative system root instead of falling back to PATH', () => {
  assert.throws(
    () => paths.powershellPath({ SystemRoot: 'Windows' }),
    /SystemRoot must be an absolute Windows path/
  );
});

test('registry PowerShell runner does not use a bare executable name', () => {
  const source = fs.readFileSync(path.join(__dirname, '..', 'install', 'paths.js'), 'utf8');
  assert.match(source, /execFileSync\(powershellPath\(env\),/);
  assert.doesNotMatch(source, /execFileSync\(\s*['"]powershell(?:\.exe)?['"]/);
});
