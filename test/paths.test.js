'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
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
