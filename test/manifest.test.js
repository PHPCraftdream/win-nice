'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const manifest = require('../install/manifest');

function tmpDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'win-nice-manifest-'));
}

test('write/read roundtrip', () => {
  const dir = tmpDir();
  const file = path.join(dir, 'install-manifest.json');
  const data = { version: '0.1.0', files: ['a.bat', 'b.ps1'] };
  manifest.write(file, data);
  assert.deepEqual(manifest.read(file), data);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('read returns null when the manifest is missing', () => {
  const dir = tmpDir();
  assert.equal(manifest.read(path.join(dir, 'nope.json')), null);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('read returns null on corrupt JSON instead of throwing', () => {
  const dir = tmpDir();
  const file = path.join(dir, 'install-manifest.json');
  fs.writeFileSync(file, '{not json');
  assert.equal(manifest.read(file), null);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('hasMarker detects the win-nice marker comment', () => {
  const dir = tmpDir();
  const marked = path.join(dir, 'marked.bat');
  const unmarked = path.join(dir, 'unmarked.bat');
  fs.writeFileSync(marked, '@echo off\r\n:: win-nice: managed-file\r\necho hi\r\n');
  fs.writeFileSync(unmarked, '@echo off\r\necho hi\r\n');
  assert.equal(manifest.hasMarker(marked), true);
  assert.equal(manifest.hasMarker(unmarked), false);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('hasMarker returns false for a missing file instead of throwing', () => {
  assert.equal(manifest.hasMarker('C:\\definitely\\not\\here.bat'), false);
});
