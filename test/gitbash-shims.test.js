'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

// Real Git Bash integration tests for the extensionless bin/<tool> shims.
// Pester can't cover these (no bash guarantee inside a plain PowerShell run),
// so each test shells out to bash and skips cleanly when bash isn't on PATH.
// Every spawn carries a hard timeout: a shim that lets MSYS mangle e.g. /c
// into a path drops the user into an interactive cmd.exe and never returns -
// that must fail the test, not hang the suite.

const REPO = path.join(__dirname, '..');
const BIN = path.join(REPO, 'bin');
const TIMEOUT_MS = 30000;

const bashProbe = spawnSync('bash', ['-c', 'echo ok'], { encoding: 'utf8' });
const HAS_BASH = bashProbe.status === 0 && bashProbe.stdout.trim() === 'ok';
const SKIP = HAS_BASH ? false : 'bash not on PATH';

function shimPath(name) {
  // Forward slashes: Git Bash handles both, but $0/dirname stay predictable.
  return path.join(BIN, name).replace(/\\/g, '/');
}

function runShim(name, args) {
  return spawnSync('bash', [shimPath(name), ...args], {
    encoding: 'utf8',
    cwd: REPO,
    timeout: TIMEOUT_MS,
  });
}

test('idle runs "cmd.exe /c exit 7" to completion and propagates exit code 7', { skip: SKIP }, () => {
  const res = runShim('idle', ['cmd.exe', '/c', 'exit', '7']);
  assert.equal(res.status, 7, `stderr: ${res.stderr}`);
});

test('idle keeps /d and /c intact as cmd.exe switches', { skip: SKIP }, () => {
  const res = runShim('idle', ['cmd.exe', '/d', '/c', 'exit', '5']);
  assert.equal(res.status, 5, `stderr: ${res.stderr}`);
});

test('idle forwards /c, /d, a Windows path, %, &, spaces and an empty string byte-exact', { skip: SKIP }, () => {
  const args = ['/c', '/d', 'C:\\Windows', 'with space', '&', '%', ''];
  const printer = 'process.stdout.write(process.argv.slice(1).map(function (a) { return JSON.stringify(a); }).join("\\n"))';
  const res = runShim('idle', ['node', '-e', printer, ...args]);
  assert.equal(res.status, 0, `stderr: ${res.stderr}`);
  const lines = res.stdout.split('\n').filter((line) => line !== '');
  assert.deepEqual(lines.map((line) => JSON.parse(line)), args);
});

test('cap receives its numeric first argument and propagates the exit code', { skip: SKIP }, () => {
  const res = runShim('cap', ['90', 'cmd.exe', '/c', 'exit', '3']);
  assert.equal(res.status, 3, `stderr: ${res.stderr}`);
});

test('pint receives its numeric first argument and propagates the exit code', { skip: SKIP }, () => {
  const res = runShim('pint', ['1', 'cmd.exe', '/c', 'exit', '4']);
  assert.equal(res.status, 4, `stderr: ${res.stderr}`);
});

test('shim resolves its .ps1 sibling when invoked as ./<tool> from bin/', { skip: SKIP }, () => {
  const res = spawnSync('bash', ['./idle', 'cmd.exe', '/c', 'exit', '2'], {
    encoding: 'utf8',
    cwd: BIN,
    timeout: TIMEOUT_MS,
  });
  assert.equal(res.status, 2, `stderr: ${res.stderr}`);
});
